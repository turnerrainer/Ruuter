use crate::{Result, RuuterError};
use reqwest::{Client, Method, Response};
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

pub struct HttpClient {
    client: Client,
    default_timeout: Duration,
}

impl HttpClient {
    pub fn new(default_timeout_ms: u64) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_millis(default_timeout_ms))
            .build()
            .unwrap();

        Self {
            client,
            default_timeout: Duration::from_millis(default_timeout_ms),
        }
    }

    pub async fn request(
        &self,
        method: Method,
        url: &str,
        body: Option<&HashMap<String, Value>>,
        query: Option<&HashMap<String, Value>>,
        headers: Option<&HashMap<String, Value>>,
        timeout: Option<Duration>,
    ) -> Result<HttpResponse> {
        let mut request = self.client
            .request(method, url)
            .timeout(timeout.unwrap_or(self.default_timeout));

        if let Some(q) = query {
            for (k, v) in q {
                request = request.query(&[(k.as_str(), v.to_string())]);
            }
        }

        if let Some(h) = headers {
            for (k, v) in h {
                request = request.header(k.as_str(), v.to_string());
            }
        }

        if let Some(b) = body {
            request = request.json(b);
        }

        let response = request.send().await?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = response.json::<Value>().await.ok();

        Ok(HttpResponse {
            status,
            body,
            headers: headers.iter()
                .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                .collect(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Option<Value>,
    pub headers: HashMap<String, String>,
}
