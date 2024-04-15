use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::log::{Level, LogPlugin};
use bevy::prelude::*;
use bevy_stream_dingtalk::prelude::StreamDingTalkPlugin;

fn main() {
    let client_id = std::env::args().nth(1).unwrap();
    let client_secret = std::env::args().nth(2).unwrap();
    App::new()
        .add_plugins(
            MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(
                1.0 / 60.0,
            ))),
        )
        .add_plugins(LogPlugin {
            level: Level::INFO,
            filter: "bevy_stream_dingtalk=debug".to_string(),
            update_subscriber: None,
        })
        .add_plugins(StreamDingTalkPlugin {
            client_id,
            client_secret,
        })
        .run();
}
