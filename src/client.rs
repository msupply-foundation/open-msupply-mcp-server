//! Open mSupply GraphQL client with authentication, token caching, and 401 retry.
//!
//! Port of src/client.ts. Uses reqwest + raw GraphQL query strings.

use crate::config::Config;
use crate::error::AppError;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::RwLock;

const AUTH_TOKEN_QUERY: &str = r#"
  query authToken($username: String!, $password: String!) {
    authToken(password: $password, username: $username) {
      ... on AuthTokenError {
        __typename
        error { description }
      }
      ... on AuthToken {
        __typename
        token
      }
    }
  }
"#;

pub struct OmSupplyClient {
    http: reqwest::Client,
    graphql_url: String,
    username: String,
    password: String,
    default_store_id: RwLock<Option<String>>,
    token: RwLock<Option<String>>,
}

impl OmSupplyClient {
    pub fn new(config: Config) -> Result<Self, AppError> {
        let mut builder = reqwest::Client::builder();
        if config.allow_self_signed {
            builder = builder.danger_accept_invalid_certs(true);
        }
        let http = builder.build().map_err(AppError::Request)?;

        let graphql_url = format!("{}/graphql", config.url);

        Ok(Self {
            http,
            graphql_url,
            username: config.username,
            password: config.password,
            default_store_id: RwLock::new(config.store_id),
            token: RwLock::new(None),
        })
    }

    pub async fn get_store_id(&self) -> Option<String> {
        self.default_store_id.read().await.clone()
    }

    /// Resolve an effective store ID: caller-supplied override > default > error.
    pub async fn require_store_id(&self, provided: Option<String>) -> Result<String, AppError> {
        if let Some(id) = provided.filter(|s| !s.is_empty()) {
            return Ok(id);
        }
        self.default_store_id
            .read()
            .await
            .clone()
            .ok_or(AppError::StoreIdRequired)
    }

    /// Run a GraphQL query, returning the deserialized `data` field.
    /// Handles lazy authentication and single 401 retry.
    pub async fn query<T: DeserializeOwned>(
        &self,
        document: &str,
        variables: Value,
    ) -> Result<T, AppError> {
        // Lazy auth
        if self.token.read().await.is_none() {
            self.authenticate().await?;
        }

        match self.send_query::<T>(document, &variables).await {
            Ok(data) => Ok(data),
            Err(AppError::Auth(_)) => {
                // Token expired or invalid -- re-auth and retry once
                *self.token.write().await = None;
                self.authenticate().await?;
                self.send_query::<T>(document, &variables).await
            }
            Err(e) => Err(e),
        }
    }

    async fn authenticate(&self) -> Result<(), AppError> {
        let body = json!({
            "query": AUTH_TOKEN_QUERY,
            "variables": {
                "username": self.username,
                "password": self.password,
            }
        });

        let response = self
            .http
            .post(&self.graphql_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::from_request(e, &self.graphql_url))?;

        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| AppError::from_request(e, &self.graphql_url))?;

        let json: Value = serde_json::from_str(&text).map_err(|e| {
            AppError::UnexpectedResponse(format!(
                "HTTP {status}: failed to parse JSON: {e}. Body: {text}"
            ))
        })?;

        // Detect "server still initialising" -- schema missing authToken field
        if let Some(errors) = json.get("errors").and_then(|e| e.as_array()) {
            let combined = errors
                .iter()
                .filter_map(|e| e.get("message").and_then(|m| m.as_str()))
                .collect::<Vec<_>>()
                .join("; ");

            if combined.contains("Unknown field") && combined.contains("authToken") {
                return Err(AppError::ServerInitialising {
                    url: self.graphql_url.clone(),
                });
            }
            return Err(AppError::Graphql(combined));
        }

        let auth_token = json
            .get("data")
            .and_then(|d| d.get("authToken"))
            .ok_or_else(|| {
                AppError::UnexpectedResponse(format!("missing data.authToken: {text}"))
            })?;

        let typename = auth_token
            .get("__typename")
            .and_then(|t| t.as_str())
            .unwrap_or("");

        match typename {
            "AuthToken" => {
                let token = auth_token
                    .get("token")
                    .and_then(|t| t.as_str())
                    .ok_or_else(|| AppError::UnexpectedResponse("missing token field".into()))?
                    .to_string();
                *self.token.write().await = Some(token);
                Ok(())
            }
            "AuthTokenError" => {
                let description = auth_token
                    .get("error")
                    .and_then(|e| e.get("description"))
                    .and_then(|d| d.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                Err(AppError::Auth(description))
            }
            other => Err(AppError::UnexpectedResponse(format!(
                "unexpected authToken __typename: {other}"
            ))),
        }
    }

    async fn send_query<T: DeserializeOwned>(
        &self,
        document: &str,
        variables: &Value,
    ) -> Result<T, AppError> {
        let token = self
            .token
            .read()
            .await
            .clone()
            .ok_or_else(|| AppError::Auth("no token available".into()))?;

        let body = json!({
            "query": document,
            "variables": variables,
        });

        let response = self
            .http
            .post(&self.graphql_url)
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::from_request(e, &self.graphql_url))?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(AppError::Auth(format!("HTTP {status}")));
        }

        let text = response
            .text()
            .await
            .map_err(|e| AppError::from_request(e, &self.graphql_url))?;

        let json: Value = serde_json::from_str(&text).map_err(|e| {
            AppError::UnexpectedResponse(format!(
                "HTTP {status}: failed to parse JSON: {e}. Body: {text}"
            ))
        })?;

        if let Some(errors) = json.get("errors").and_then(|e| e.as_array()) {
            let combined = errors
                .iter()
                .filter_map(|e| e.get("message").and_then(|m| m.as_str()))
                .collect::<Vec<_>>()
                .join("; ");
            if combined.to_lowercase().contains("unauthenticated") || combined.contains("401") {
                return Err(AppError::Auth(combined));
            }
            return Err(AppError::Graphql(combined));
        }

        let data = json
            .get("data")
            .cloned()
            .ok_or_else(|| AppError::UnexpectedResponse(format!("missing data: {text}")))?;

        serde_json::from_value::<T>(data)
            .map_err(|e| AppError::UnexpectedResponse(format!("failed to deserialize: {e}")))
    }
}
