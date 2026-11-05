use super::*;

pub(crate) struct WasmProxyClient {
    client: reqwest::Client,
}

impl WasmProxyClient {
    pub(crate) fn new() -> Result<Self, Error> {
        let client = reqwest::Client::new();
        Ok(Self { client })
    }

    pub(crate) async fn get_reject_list(&self, url: &str) -> Result<String, Error> {
        Ok(self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| Error::RejectListService {
                details: e.to_string(),
            })?
            .text()
            .await
            .map_err(InternalError::from)?)
    }
}
