use super::*;

/// Maximum size, in bytes, of a reject list line: the allow prefix (`!`), the opout
/// (`{opid}/{ty}/{no}`, a hex opid and two decimal `u16`s) and a CRLF line ending
const MAX_REJECT_LIST_LINE_SIZE: u64 = 1 + 64 + 1 + 5 + 1 + 5 + 2;
/// Maximum size, in bytes, of a reject list served at a URL (see [`MAX_REJECT_LIST_ENTRIES`])
const MAX_REJECT_LIST_SIZE: u64 = MAX_REJECT_LIST_ENTRIES * MAX_REJECT_LIST_LINE_SIZE;

pub(crate) struct RejectListClient {
    base_url: String,
}

impl RejectListClient {
    /// A client for the reject list served at `base_url`.
    ///
    /// The URL comes from the asset genesis, so it is not trusted. Only `http` and `https` URLs
    /// are accepted and redirects are not followed.
    pub(crate) fn new(base_url: &str) -> Result<Self, Error> {
        let url = reqwest::Url::parse(base_url).map_err(Self::req_err)?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(Self::req_err(format!(
                "unsupported reject list URL scheme: {}",
                url.scheme()
            )));
        }
        Ok(Self {
            base_url: base_url.to_string(),
        })
    }

    fn req_err(e: impl std::fmt::Display) -> Error {
        Error::RejectListService {
            details: e.to_string(),
        }
    }

    fn status_err(status: impl std::fmt::Display) -> Error {
        Self::req_err(format!("reject list request failed: HTTP {status}"))
    }

    /// Get the reject list. An error page or a redirect is refused, as an empty list would allow
    /// every opout. A body larger than [`MAX_REJECT_LIST_SIZE`] is refused.
    #[cfg(target_arch = "wasm32")]
    pub(crate) async fn get_reject_list(&self) -> Result<String, Error> {
        use wasm_bindgen::JsCast;
        let resp = gloo_net::http::Request::get(&self.base_url)
            .redirect(web_sys::RequestRedirect::Error)
            .send()
            .await
            .map_err(Self::req_err)?;
        if !resp.ok() {
            return Err(Self::status_err(resp.status()));
        }
        let mut body = vec![];
        if let Some(stream) = resp.body() {
            let reader: web_sys::ReadableStreamDefaultReader = stream.get_reader().unchecked_into();
            let js_err = |e: wasm_bindgen::JsValue| Self::req_err(format!("{e:?}"));
            // read one byte past the bound to tell a body of the maximum size from a longer one
            while body.len() as u64 <= MAX_REJECT_LIST_SIZE {
                let chunk = wasm_bindgen_futures::JsFuture::from(reader.read())
                    .await
                    .map_err(js_err)?;
                if js_sys::Reflect::get(&chunk, &"done".into())
                    .map_err(js_err)?
                    .is_truthy()
                {
                    break;
                }
                let value = js_sys::Reflect::get(&chunk, &"value".into()).map_err(js_err)?;
                body.extend(js_sys::Uint8Array::new(&value).to_vec());
            }
            let _ = reader.cancel();
        }
        Self::checked_body(body)
    }

    /// Get the reject list (native tests only, see the wasm32 version).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) async fn get_reject_list(&self) -> Result<String, Error> {
        let mut resp = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(Self::req_err)?
            .get(&self.base_url)
            .send()
            .await
            .map_err(Self::req_err)?;
        if !resp.status().is_success() {
            return Err(Self::status_err(resp.status()));
        }
        let mut body = vec![];
        while body.len() as u64 <= MAX_REJECT_LIST_SIZE {
            match resp.chunk().await.map_err(Self::req_err)? {
                Some(chunk) => body.extend(chunk),
                None => break,
            }
        }
        Self::checked_body(body)
    }

    fn checked_body(body: Vec<u8>) -> Result<String, Error> {
        if body.len() as u64 > MAX_REJECT_LIST_SIZE {
            return Err(Self::req_err(format!(
                "reject list too large: more than {MAX_REJECT_LIST_SIZE} bytes"
            )));
        }
        Ok(String::from_utf8_lossy(&body).into_owned())
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::api::evm::tests::block_on;
    use std::io::Write;

    fn get(url: &str) -> Result<String, Error> {
        block_on(RejectListClient::new(url).unwrap().get_reject_list())
    }

    #[test]
    fn get_reject_list_error() {
        // network error
        assert!(matches!(
            get("http://127.0.0.1:1"),
            Err(Error::RejectListService { .. })
        ));

        // error pages and redirects (not followed) are refused, not taken as an empty list
        for status in [301, 302, 404, 500] {
            let mut server = mockito::Server::new();
            let mock = server
                .mock("GET", "/")
                .with_status(status)
                .with_header("location", "http://127.0.0.1:1/elsewhere")
                .with_body("not a reject list")
                .create();
            let result = get(&server.url());
            assert!(
                matches!(result, Err(Error::RejectListService { ref details }) if details.contains(&status.to_string())),
                "{result:?}"
            );
            mock.assert();
        }

        // URLs other than http(s) ones are refused
        for url in ["file:///etc/passwd", "ftp://127.0.0.1/list", "not a url"] {
            assert!(matches!(
                RejectListClient::new(url).err(),
                Some(Error::RejectListService { .. })
            ));
        }
    }

    #[test]
    fn reject_list_line_size() {
        // the longest line is an allowed opout with the largest assignment type and output number
        let opout = Opout::new(
            OpId::from([0xff; 32]),
            AssignmentType::with(u16::MAX),
            u16::MAX,
        );
        let line = format!("!{opout}\r\n");
        assert_eq!(line.len() as u64, MAX_REJECT_LIST_LINE_SIZE);
        assert_eq!(Opout::from_str(line[1..].trim_end()).unwrap(), opout);
    }

    #[test]
    fn get_reject_list_size() {
        // a list of the maximum size is read
        let mut server = mockito::Server::new();
        let body = "a".repeat(MAX_REJECT_LIST_SIZE as usize);
        let mock = server.mock("GET", "/").with_body(&body).create();
        assert_eq!(get(&server.url()).unwrap(), body);
        mock.assert();

        // a longer one is refused, with or without a content length
        for chunked in [false, true] {
            let mut server = mockito::Server::new();
            let body = "a".repeat(MAX_REJECT_LIST_SIZE as usize + 1);
            let mock = if chunked {
                server
                    .mock("GET", "/")
                    .with_chunked_body(move |w| w.write_all(body.as_bytes()))
            } else {
                server.mock("GET", "/").with_body(&body)
            }
            .create();
            let result = get(&server.url());
            assert!(
                matches!(result, Err(Error::RejectListService { ref details }) if details.contains("reject list too large")),
                "{result:?}"
            );
            mock.assert();
        }
    }
}
