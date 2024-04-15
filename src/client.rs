use anyhow::{bail, Result};
use bevy::prelude::{debug, Resource, States};
use chrono::{DateTime, Local};
use reqwest::header::ACCEPT;
use serde::{Deserialize, Serialize};

use crate::constant::{GATEWAY_URL, GET_TOKEN_URL};

#[derive(Debug, Resource)]
pub struct DingTalkClient {
    pub config: ClientConfig,
    http_client: reqwest::blocking::Client,
    access_token: String,
    token_expires_in: DateTime<Local>,
}

impl DingTalkClient {
    pub fn new(client_id: String, client_secret: String) -> Self {
        Self {
            config: ClientConfig {
                client_id,
                client_secret,
                ..Default::default()
            },
            http_client: reqwest::blocking::Client::new(),
            access_token: "".to_string(),
            token_expires_in: Default::default(),
        }
    }

    pub fn get_token(&mut self) -> Result<String> {
        let url = format!(
            "{GET_TOKEN_URL}?appkey={}&appsecret={}",
            self.config.client_id, self.config.client_secret
        );

        let response = self.http_client.get(url).send()?;
        if !response.status().is_success() {
            bail!(
                "get token http error: {} - {}",
                response.status(),
                response.text()?
            );
        }

        let token: TokenResponse = response.json()?;
        if token.errcode != 0 {
            bail!(
                "get token content error: {} - {}",
                token.errcode,
                token.errmsg
            );
        }

        debug!("get token: {:?}", token);
        let access_token = token.access_token;
        self.access_token = access_token.clone();
        self.token_expires_in = Local::now() + chrono::Duration::seconds(token.expires_in as i64);

        Ok(access_token)
    }

    pub fn get_endpoint(&mut self) -> Result<String> {
       let token =   self.get_token()?;
        let response = self
            .http_client
            .post(GATEWAY_URL)
            .json(&self.config)
            .header(ACCEPT, "application/json")
            .header("access-token", token)
            .send()?;
        if !response.status().is_success() {
            bail!(
                "get endpoint http error: {} - {}",
                response.status(),
                response.text()?
            );
        }

        let endpoint: EndpointResponse = response.json()?;
        debug!("get endpoint: {:?}", endpoint);
        let EndpointResponse { endpoint, ticket } = endpoint;

        Ok(format!("{endpoint}?ticket={ticket}"))
    }
}

/// Client config that need to be sent to DingTalk server to get endpoint
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientConfig {
    /// Client id also known as AppKey in DingTalk Backend
    pub client_id: String,
    /// Client secret also known as AppSecret in DingTalk Backend
    pub client_secret: String,
    /// User-Agent sent to server
    pub ua: String,
    /// Subscriptions defines the types of event that you are concerned about
    pub subscriptions: Vec<Subscription>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            client_id: Default::default(),
            client_secret: Default::default(),
            ua: Default::default(),
            subscriptions: vec![
                Subscription {
                    r#type: "EVENT".to_owned(),
                    topic: "*".to_owned(),
                },
                Subscription {
                    r#type: "SYSTEM".to_owned(),
                    topic: "*".to_owned(),
                },
            ],
        }
    }
}

/// Definition of subscription types registered with the DingTalk server
#[derive(Debug, Serialize)]
pub struct Subscription {
    /// Type
    /// - EVENT
    /// - SYSTEM
    /// - CALLBACK
    pub r#type: String,
    /// Topic
    /// - "/v1.0/im/bot/messages/get";
    /// - "/v1.0/card/instances/callback";
    pub topic: String,
}

#[derive(Deserialize, Debug)]
struct TokenResponse {
    errcode: u32,
    access_token: String,
    errmsg: String,
    expires_in: u32,
}

#[derive(States, Debug, Default, Clone, PartialEq, Eq, Hash)]
pub enum ConnectionState {
    Connected,
    Connecting,
    #[default]
    Disconnected,
}

#[derive(Debug, Deserialize)]
struct EndpointResponse {
    endpoint: String,
    ticket: String,
}