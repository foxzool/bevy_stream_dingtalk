use std::ops::Deref;
use anyhow::{bail, Result};
use bevy::prelude::{debug, Deref, DerefMut, FromWorld, Resource, States, World};
use chrono::{DateTime, Duration, Local};
use serde::{Deserialize, Serialize};



use async_broadcast::{Receiver, Sender};

use bevy::log::{error, info, trace, warn};
use down::{ClientDownStream, EventData, RobotRecvMessage};
use futures::{stream::SplitStream, Future, StreamExt};
use native_tls::TlsConnector;
use reqwest::{header::ACCEPT, ClientBuilder};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, RwLock,
};
use tokio::{net::TcpStream, runtime, sync::Notify, time::sleep};
use tokio_tungstenite::{
    connect_async_tls_with_config,
    tungstenite::{Error, Message},
    Connector, MaybeTlsStream, WebSocketStream,
};
use up::{EventAckData, Sink};

use crate::constant::{GATEWAY_URL, GET_TOKEN_URL};

pub mod down;
pub mod group;
pub mod up;

#[derive(Debug, Resource, Deref, DerefMut)]
pub struct AsyncRuntime(pub tokio::runtime::Runtime);

#[derive(States, Debug, Default, Clone, PartialEq, Eq, Hash)]
pub enum ConnectionState {
    Connected,
    Connecting,
    #[default]
    Disconnected,
}

#[derive(Resource)]
pub struct DingTalkClient {
    client: Arc<Client>
}

impl DingTalkClient {
    pub fn new(client_id: impl Into<String>, client_secret: impl Into<String>) -> Result<Self> {
        Ok(Self {
            client: Client::new(client_id, client_secret)?
        })
    }
}

impl Deref for DingTalkClient {
    type Target = Arc<Client>;

    fn deref(&self) -> &Self::Target {
        &self.client
    }
}

/// An asynchronous [`Client`] to interactive with DingTalk server
///
/// Using websocket for receiving message and https for sending
#[derive(Debug, Resource)]
pub struct Client {
    /// config inside client can be adjusted
    pub config: Arc<Mutex<ClientConfig>>,
    client: reqwest::Client,
    rx: Receiver<Arc<ClientDownStream>>,
    tx: Sender<Arc<ClientDownStream>>,
    on_event_callback: EventCallback,
    sink: tokio::sync::Mutex<Option<Sink>>,
    alive: AtomicBool,
    user_exit: AtomicBool,
    aborting: Arc<Notify>,
}

struct EventCallback(RwLock<Box<dyn Fn(EventData) -> EventAckData + Send + Sync>>);

impl std::fmt::Debug for EventCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("EventCallback").finish()
    }
}

impl Client {
    /// Create new client, need to specific the id and secret they provided when creating the robot
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Result<Arc<Self>> {
        let client_id = client_id.into();
        let client_secret = client_secret.into();
        let (tx, rx) = async_broadcast::broadcast(32);
        Ok(Arc::new(Self {
            config: Arc::new(Mutex::new(ClientConfig {
                client_id,
                client_secret,
                ..Default::default()
            })),
            client: ClientBuilder::new()
                .no_proxy()
                .danger_accept_invalid_certs(true)
                .build()?,
            tx,
            rx,
            sink: tokio::sync::Mutex::new(None),
            on_event_callback: EventCallback(RwLock::new(Box::new(|p| {
                info!("default event callback, event received: {:?}", p);
                EventAckData::default()
            }))),
            alive: AtomicBool::new(false),
            user_exit: AtomicBool::new(false),
            aborting: Arc::new(Notify::new()),
        }))
    }

    /// Change the User-Agent
    pub fn ua(self: Arc<Self>, value: impl Into<String>) -> Arc<Self> {
        self.config.lock().unwrap().ua = value.into();
        self
    }

    /// Control client side keep alive heartbeat interval(ms), default is 8000.
    /// When set to 0, means disable keep alive heartbeat.
    pub fn keep_alive(self: Arc<Self>, value: i64) -> Arc<Self> {
        self.config.lock().unwrap().heartbeat_interval = value;
        self
    }

    /// Control client reconnect when websocket disconnected(ms), default is 1000ms.
    /// When set to 0, means disable reconnect.
    pub fn reconnect(self: Arc<Self>, value: i64) -> Arc<Self> {
        self.config.lock().unwrap().reconnect_interval = value;
        self
    }

    /// Add listener to watch all event.
    /// Calling this interface multiple times will replace the old listener with a new one.
    pub fn register_all_event_listener<P>(self: Arc<Self>, on_event_received: P) -> Arc<Self>
    where
        P: Fn(EventData) -> EventAckData + Send + Sync + 'static,
    {
        *self.on_event_callback.0.write().unwrap() = Box::new(on_event_received);
        self
    }

    /// Add listener to watch specifc event id
    pub fn register_callback_listener<P, F>(
        self: Arc<Self>,
        event_id: impl AsRef<str>,
        callback: P,
    ) -> Arc<Self>
    where
        P: Fn(Arc<Self>, RobotRecvMessage) -> F + Send + 'static,
        F: Future<Output = Result<()>> + Send,
    {
        let event_id = event_id.as_ref();
        {
            let mut config = self.config.lock().unwrap();
            if !config
                .subscriptions
                .iter()
                .any(|s| s.topic == event_id && s.r#type == "CALLBACK")
            {
                config.subscriptions.push(Subscription {
                    topic: event_id.to_owned(),
                    r#type: "CALLBACK".to_owned(),
                });
            }
        }

        tokio::spawn({
            let mut rx = self.rx.clone();
            let s = self.clone();
            async move {
                while let Ok(msg) = rx.recv().await {
                    match serde_json::from_str(&msg.data) {
                        Ok(msg) => {
                            if let Err(e) = callback(s.clone(), msg).await {
                                error!("callback error: {:?}", e);
                            }
                        }
                        Err(e) => {
                            error!("can not parse data: {:?}", e);
                        }
                    }
                }
            }
        });

        self
    }

    pub(crate) async fn token(&self) -> Result<String> {
        let (access_token, token_expires_in) = {
            let config = self.config.lock().unwrap();
            (config.access_token.clone(), config.token_expires_in)
        };

        Ok(if Local::now() > token_expires_in {
            debug!("token expired, get token again");
            self.get_token().await?
        } else {
            access_token
        })
    }

    async fn get_token(&self) -> Result<String> {
        let url = {
            let config = self.config.lock().unwrap();
            debug!("get connect endpoint by config {:#?}", *config);
            format!(
                "{GET_TOKEN_URL}?appkey={}&appsecret={}",
                config.client_id, config.client_secret
            )
        };
        let response = self.client.get(url).send().await?;
        if !response.status().is_success() {
            bail!(
                "get token http error: {} - {}",
                response.status(),
                response.text().await?
            );
        }

        let token: TokenResponse = response.json().await?;
        if token.errcode != 0 {
            bail!(
                "get token content error: {} - {}",
                token.errcode,
                token.errmsg
            );
        }

        debug!("get token: {:?}", token);
        let access_token = token.access_token;
        let mut config = self.config.lock().unwrap();
        config.access_token = access_token.clone();
        config.token_expires_in = Local::now() + Duration::seconds(token.expires_in as i64);
        Ok(access_token)
    }

    async fn get_endpoint(&self) -> Result<String> {
        let token = self.get_token().await?;

        let response = self
            .client
            .post(GATEWAY_URL)
            .json(&*self.config)
            .header(ACCEPT, "application/json")
            .header("access-token", token)
            .send()
            .await?;
        if !response.status().is_success() {
            bail!(
                "get endpoint http error: {} - {}",
                response.status(),
                response.text().await?
            );
        }

        let endpoint: EndpointResponse = response.json().await?;
        debug!("get endpoint: {:?}", endpoint);
        let EndpointResponse { endpoint, ticket } = endpoint;

        Ok(format!("{endpoint}?ticket={ticket}"))
    }

    async fn serve(self: &Arc<Self>, url: String) -> Result<()> {
        let tls_connect = Connector::NativeTls({
            TlsConnector::builder()
                .danger_accept_invalid_certs(true)
                .danger_accept_invalid_hostnames(true)
                .build()?
        });

        let (stream, _) =
            match connect_async_tls_with_config(&url, None, false, Some(tls_connect)).await {
                Ok(x) => {
                    self.alive.store(true, Ordering::SeqCst);
                    x
                }
                Err(e) => {
                    if let Error::Http(ref h) = e {
                        bail!(
                            "connect websocket http error: {} - {}",
                            h.status(),
                            String::from_utf8_lossy(h.body().as_deref().unwrap_or_default())
                        );
                    } else {
                        bail!("connect websocket error: {:?}", e);
                    }
                }
            };

        let (sink, stream) = stream.split();
        *self.sink.lock().await = Some(sink);
        let heartbeat_interval = self.config.lock().unwrap().heartbeat_interval;
        if heartbeat_interval > 0 {
            tokio::spawn({
                let s = self.clone();
                let aborting = self.aborting.clone();
                async move {
                    loop {
                        if !s.alive.load(Ordering::SeqCst) {
                            aborting.notify_one();
                            break;
                        }

                        trace!("websocket ping");
                        s.alive.store(false, Ordering::SeqCst);
                        let _ = s.ping().await;
                        // heartbeat_interval is always larger than zero, to_std() never failed. unwrap is safe here
                        sleep(Duration::milliseconds(heartbeat_interval).to_std().unwrap()).await;
                    }
                }
            });
        }

        tokio::select! {
            _ = self.aborting.notified() => { warn!("server aborting"); }
            _ = self.process(stream) => { warn!("server error or closed"); }
        }

        self.alive.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn process(
        &self,
        mut stream: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    ) -> Result<()> {
        while let Some(message) = stream.next().await {
            let message = match message {
                Ok(m) => m,
                Err(e) => {
                    error!("recv websocket message error: {:?}", e);
                    break;
                }
            };

            match message {
                Message::Text(t) => {
                    debug!("recv websocket text: {t}");
                    match serde_json::from_str::<ClientDownStream>(&t) {
                        Ok(p) => self.on_down_stream(p).await?,
                        Err(e) => {
                            warn!("parse websocket text error: {:?}", e)
                        }
                    }
                }
                Message::Pong(_) => {
                    trace!("websocket pong");
                    self.alive.store(true, Ordering::SeqCst)
                }
                Message::Close(c) => {
                    warn!(
                        "Websocket closed: {}",
                        if let Some(c) = c {
                            c.to_string()
                        } else {
                            "Unknown reason".to_owned()
                        }
                    );

                    break;
                }

                _ => {
                    warn!("Unhandled websocket message: {:?}", message)
                }
            }
        }

        Ok(())
    }

    /// Connect to api gateway, and begin the websocket stream process
    pub async fn connect(self: Arc<Self>) -> Result<()> {
        loop {
            let c = self.clone();
            let reconnect_interval = c.config.lock().unwrap().reconnect_interval;
            let url = c.get_endpoint().await?;
            c.serve(url).await?;

            if reconnect_interval > 0 && !self.user_exit.load(Ordering::SeqCst) {
                info!("Reconnecting in {} seconds...", reconnect_interval / 1000);

                // reconnect_interval is always larger than zero, to_std() never failed. unwrap is safe here
                sleep(Duration::milliseconds(reconnect_interval).to_std().unwrap()).await;
                debug!("initial reconnecting...");
            } else {
                break;
            }
        }

        Ok(())
    }

    pub fn exit(&self) {
        self.user_exit.store(true, Ordering::SeqCst);
        self.aborting.notify_waiters();
    }
}

#[derive(Deserialize, Debug)]
struct TokenResponse {
    errcode: u32,
    access_token: String,
    errmsg: String,
    expires_in: u32,
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
    #[serde(skip_serializing)]
    access_token: String,
    #[serde(skip_serializing)]
    token_expires_in: DateTime<Local>,
    #[serde(skip_serializing)]
    reconnect_interval: i64,
    #[serde(skip_serializing)]
    heartbeat_interval: i64,
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
            access_token: String::new(),
            token_expires_in: Local::now(),
            reconnect_interval: 1000,
            heartbeat_interval: 8000,
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

#[derive(Debug, Deserialize)]
struct EndpointResponse {
    endpoint: String,
    ticket: String,
}
