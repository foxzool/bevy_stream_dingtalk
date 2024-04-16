//! Types and methods that handle up to DingTalk server



use crate::client::Client;
use anyhow::{bail, Result};
use futures::{stream::SplitSink, SinkExt};
use log::debug;
use reqwest::{
    multipart::{Form, Part},
    Response,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::{ffi::OsStr, path::Path, sync::Arc};
use strum::Display;
use tokio::{fs::File, net::TcpStream};
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};

pub(crate) type Sink = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;
impl Client {
    pub(crate) async fn send<T: Serialize>(&self, msg: T) -> Result<()> {
        let msg = serde_json::to_string(&msg)?;
        self.send_message(Message::text(msg)).await
    }

    pub(crate) async fn ping(&self) -> Result<()> {
        self.send_message(Message::Ping(Vec::new())).await
    }

    pub(crate) async fn send_message(&self, msg: Message) -> Result<()> {
        let mut sink = self.sink.lock().await;
        let Some(sink) = sink.as_mut() else {
            bail!("stream not connected");
        };
        sink.send(msg).await?;

        Ok(())
    }

    pub(crate) async fn post_raw<T: Serialize>(
        &self,
        url: impl AsRef<str>,
        data: T,
    ) -> Result<Response> {
        let access_token = self.token().await?;
        debug!("post with access token: {}", access_token);
        let response = self
            .client
            .post(url.as_ref())
            .header("x-acs-dingtalk-access-token", access_token)
            .json(&data)
            .send()
            .await?;

        if !response.status().is_success() {
            bail!(
                "post error: [{}] {:?}",
                response.status(),
                response.text().await?
            );
        }

        Ok(response)
    }

    pub(crate) async fn post<T, U>(&self, url: impl AsRef<str>, data: T) -> Result<U>
    where
        T: Serialize,
        U: DeserializeOwned,
    {
        let response = self.post_raw(url, data).await?;
        let status = response.status();
        let text = response.text().await?;
        debug!("post ok: [{}] {}", status, text);
        Ok(serde_json::from_str(&text)?)
    }

    /// upload file and return media id for
    /// - [`MessageTemplate::SampleFile`]
    /// - [`MessageTemplate::SampleVideo`]
    /// - [`MessageTemplate::SampleAudio`]
    pub async fn upload(&self, file: impl AsRef<Path>, file_type: UploadType) -> Result<String> {
        let access_token = self.token().await?;
        let file = file.as_ref();
        let filename = file
            .file_name()
            .unwrap_or(OsStr::new("<unknown>"))
            .to_string_lossy()
            .to_string();
        let file = File::open(file).await?;
        let form = Form::new()
            .part("media", Part::stream(file).file_name(filename))
            .text("type", file_type.to_string());
        let response = self
            .client
            .post(format!("{}?access_token={}", UPLOAD_URL, access_token))
            .multipart(form)
            .send()
            .await?;

        if !response.status().is_success() {
            bail!(
                "upload http error: {} - {}",
                response.status(),
                response.text().await?
            );
        }

        let res: UploadResult = response.json().await?;
        if res.errcode != 0 {
            bail!("upload error: {} - {}", res.errcode, res.errmsg);
        }

        Ok(res.media_id)
    }
}

#[derive(Deserialize)]
struct UploadResult {
    errcode: u32,
    errmsg: String,
    #[serde(default)]
    media_id: String,
    #[allow(dead_code)]
    #[serde(default)]
    created_at: u64,
    #[allow(dead_code)]
    #[serde(default)]
    r#type: String,
}

/// Upload enum for [`Client::upload`]
#[derive(Display)]
#[strum(serialize_all = "snake_case")]
pub enum UploadType {
    Image,
    Voice,
    Video,
    File,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClientUpStream {
    pub code: u32,
    pub headers: StreamUpHeader,
    pub message: String,
    pub data: String,
}

impl ClientUpStream {
    pub fn new(data: impl Into<String>, message_id: impl Into<String>) -> Self {
        let data = data.into();
        let message_id = message_id.into();

        Self {
            code: 200,
            headers: StreamUpHeader {
                message_id,
                content_type: "application/json".to_owned(),
            },
            message: "OK".to_owned(),
            data,
        }
    }
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StreamUpHeader {
    pub content_type: String, // always application/json
    pub message_id: String,   // same StreamDownHeaders::message_id
}

/// Message type to be sent to DingTalk server
///
/// Please refer to the official document [batches](https://open.dingtalk.com/document/orgapp/chatbots-send-one-on-one-chat-messages-in-batches) and
/// [group](https://open.dingtalk.com/document/orgapp/the-robot-sends-a-group-message) for more detail
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RobotSendMessage {
    robot_code: String,
    #[serde(flatten)]
    target: SendMessageTarget,
    msg_key: String,
    msg_param: String,

    #[serde(skip_serializing)]
    client: Arc<Client>,
}

const BATCH_SEND_URL: &str = "https://api.dingtalk.com/v1.0/robot/oToMessages/batchSend";
const GROUP_SEND_URL: &str = "https://api.dingtalk.com/v1.0/robot/groupMessages/send";
const UPLOAD_URL: &str = "https://oapi.dingtalk.com/media/upload";

impl RobotSendMessage {
    /// construct message to group chat
    pub fn group(
        client: Arc<Client>,
        conversation_id: impl Into<String>,
        message: MessageTemplate,
    ) -> Result<Self> {
        let client_id = client.config.lock().unwrap().client_id.clone();
        Ok(Self {
            robot_code: client_id,
            target: SendMessageTarget::Group {
                open_conversation_id: conversation_id.into(),
            },
            msg_key: message.to_string(),
            msg_param: message.try_into()?,
            client,
        })
    }

    /// send to constructed message
    pub async fn send(&self) -> Result<()> {
        debug!("send: {}", serde_json::to_string(self).unwrap());
        let _: Value = self
            .client
            .post(
                {
                    match self.target {
                        SendMessageTarget::Batch { .. } => BATCH_SEND_URL,
                        SendMessageTarget::Group { .. } => GROUP_SEND_URL,
                    }
                },
                self,
            )
            .await?;

        Ok(())
    }

    /// construct batch message to multiple users
    pub fn batch(
        client: Arc<Client>,
        user_ids: Vec<String>,
        message: MessageTemplate,
    ) -> Result<Self> {
        let client_id = client.config.lock().unwrap().client_id.clone();
        Ok(Self {
            robot_code: client_id,
            target: SendMessageTarget::Batch { user_ids },
            msg_key: message.to_string(),
            msg_param: message.try_into()?,
            client,
        })
    }

    /// construct message to single user
    pub fn single(
        client: Arc<Client>,
        user_id: impl Into<String>,
        message: MessageTemplate,
    ) -> Result<Self> {
        Self::batch(client, vec![user_id.into()], message)
    }
}

/// Event ack message type
///
/// Found it in other programming language's SDK, not found in any official document though.
#[derive(Serialize)]
pub struct EventAckData {
    pub status: &'static str,
    #[serde(default)]
    pub message: String,
}

impl Default for EventAckData {
    fn default() -> Self {
        Self {
            status: EventAckData::SUCCESS,
            message: Default::default(),
        }
    }
}

impl EventAckData {
    pub const SUCCESS: &'static str = "SUCCESS";
    pub const LATER: &'static str = "LATER";
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase", untagged)]
enum SendMessageTarget {
    #[serde(rename_all = "camelCase")]
    Group { open_conversation_id: String },
    #[serde(rename_all = "camelCase")]
    Batch { user_ids: Vec<String> },
}

/// Message enum to be sent to DingTalk server
///
/// Please refer to the [official document](https://open.dingtalk.com/document/orgapp/types-of-messages-sent-by-robots) for the definition of each field
#[derive(Serialize, strum::Display, Clone)]
#[serde(rename_all = "camelCase", untagged)]
#[strum(serialize_all = "camelCase")]
pub enum MessageTemplate {
    #[serde(rename_all = "camelCase")]
    SampleText { content: String },
    #[serde(rename_all = "camelCase")]
    SampleMarkdown { title: String, text: String },
    #[serde(rename_all = "camelCase")]
    SampleImageMsg {
        #[serde(rename = "photoURL")]
        photo_url: String,
    },
    #[serde(rename_all = "camelCase")]
    SampleLink {
        text: String,
        title: String,
        pic_url: String,
        message_url: String,
    },
    #[serde(rename_all = "camelCase")]
    SampleActionCard {
        title: String,
        text: String,
        single_title: String,
        #[serde(rename = "singleURL")]
        single_url: String,
    },
    #[serde(rename_all = "camelCase")]
    SampleActionCard2 {
        title: String,
        text: String,
        action_title_1: String,
        #[serde(rename = "actionURL1")]
        action_url_1: String,
        action_title_2: String,
        #[serde(rename = "actionURL2")]
        action_url_2: String,
    },
    #[serde(rename_all = "camelCase")]
    SampleActionCard3 {
        title: String,
        text: String,
        action_title_1: String,
        #[serde(rename = "actionURL1")]
        action_url_1: String,
        action_title_2: String,
        #[serde(rename = "actionURL2")]
        action_url_2: String,
        action_title_3: String,
        #[serde(rename = "actionURL3")]
        action_url_3: String,
    },
    #[serde(rename_all = "camelCase")]
    SampleActionCard4 {
        title: String,
        text: String,
        action_title_1: String,
        #[serde(rename = "actionURL1")]
        action_url_1: String,
        action_title_2: String,
        #[serde(rename = "actionURL2")]
        action_url_2: String,
        action_title_3: String,
        #[serde(rename = "actionURL3")]
        action_url_3: String,
        action_title_4: String,
        #[serde(rename = "actionURL4")]
        action_url_4: String,
    },
    #[serde(rename_all = "camelCase")]
    SampleActionCard5 {
        title: String,
        text: String,
        action_title_1: String,
        #[serde(rename = "actionURL1")]
        action_url_1: String,
        action_title_2: String,
        #[serde(rename = "actionURL2")]
        action_url_2: String,
        action_title_3: String,
        #[serde(rename = "actionURL3")]
        action_url_3: String,
        action_title_4: String,
        #[serde(rename = "actionURL4")]
        action_url_4: String,
        action_title_5: String,
        #[serde(rename = "actionURL5")]
        action_url_5: String,
    },
    #[serde(rename_all = "camelCase")]
    SampleActionCard6 {
        title: String,
        text: String,
        button_title_1: String,
        button_url_1: String,
        button_title_2: String,
        button_url_2: String,
    },
    #[serde(rename_all = "camelCase")]
    SampleAudio { media_id: String, duration: String },
    #[serde(rename_all = "camelCase")]
    SampleFile {
        media_id: String,
        file_name: String,
        file_type: String,
    },
    #[serde(rename_all = "camelCase")]
    SampleVideo {
        duration: String,
        video_media_id: String,
        video_type: String,
        pic_media_id: String,
    },
}

impl TryInto<String> for MessageTemplate {
    type Error = serde_json::Error;

    fn try_into(self) -> std::result::Result<String, Self::Error> {
        serde_json::to_string(&self)
    }
}
