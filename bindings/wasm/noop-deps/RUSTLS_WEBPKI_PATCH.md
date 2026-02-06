# rustls-webpki: ошибка `Ipv4Addr::as_ref` / `Ipv6Addr::as_ref`

При сборке с глобальными `[patch.crates-io]` (no-op для WASM) может появиться ошибка в **rustls-webpki**:

```
the method `as_ref` exists for reference `&core::net::Ipv4Addr`, but its trait bounds were not satisfied
```

## Вариант 1 (предпочтительно): обновить Rust

В Rust 1.77+ для `Ipv4Addr`/`Ipv6Addr` стабилизирован `AsRef`. Обновите тулчейн:

```bash
rustup update stable
```

После этого можно закомментировать или удалить строку `rustls-webpki = { path = ... }` из `[patch.crates-io]` в корневом `Cargo.toml`, и сборка будет проходить без локального патча.

## Вариант 2: локальный патч (скрипт)

Если обновить Rust нельзя, из корня репозитория выполните:

```bash
./bindings/wasm/noop-deps/prepare-rustls-webpki-patch.sh
```

Скрипт копирует **rustls-webpki 0.103.8** из кэша Cargo в `bindings/wasm/noop-deps/rustls-webpki` и заменяет в `ip_address.rs` вызовы `ip.as_ref()` на `&ip.octets()`. Патч уже указан в корневом `Cargo.toml`; после выполнения скрипта сборка `cargo check -p rgb-lib` должна проходить.
