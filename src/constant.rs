pub const GATEWAY_URL: &str = "https://api.dingtalk.com/v1.0/gateway/connections/open";
pub const TOPIC_CALLBACK: &str = "/v1.0/im/bot/messages/get";
pub const GET_TOKEN_URL: &str = "https://oapi.dingtalk.com/gettoken";

/// used for register robot message callback
pub const TOPIC_ROBOT: &str = "/v1.0/im/bot/messages/get";
/// used for register card callback
pub const TOPIC_CARD: &str = "/v1.0/card/instances/callback";