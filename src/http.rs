//! Construcción compartida de clientes HTTP (`reqwest`) con timeout.

use std::time::Duration;

/// Builder base con timeout, para quien necesite opciones extra (ej.
/// `user_agent` en las tools web).
pub fn client_builder(timeout: Duration) -> reqwest::ClientBuilder {
    reqwest::Client::builder().timeout(timeout)
}

/// Cliente listo con solo timeout — el caso común de todos los providers.
pub fn client(timeout: Duration) -> reqwest::Client {
    client_builder(timeout)
        .build()
        .expect("configuración de cliente reqwest válida")
}
