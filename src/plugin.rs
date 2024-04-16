use std::time::Duration;

use bevy::prelude::*;
use bevy::tasks::TaskPoolBuilder;
use bevy::time::common_conditions::on_timer;
use tokio::runtime;


use crate::client::{ConnectionState, Client, DingTalkClient, AsyncRuntime};
use crate::system::*;

pub struct StreamDingTalkPlugin {
    pub client_id: String,
    pub client_secret: String,
}

impl Plugin for StreamDingTalkPlugin {
    fn build(&self, app: &mut App) {

        debug!(
            "StreamDingTalkPlugin init with client_id: {}, client_secret: {}",
            self.client_id, self.client_secret
        );
        let async_runtime = runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        app
            .insert_resource(AsyncRuntime(async_runtime))
            .insert_resource(DingTalkClient::new(
                    self.client_id.clone(),
                    self.client_secret.clone(),
                ).unwrap()
            )
        .init_state::<ConnectionState>();
        app.add_systems(
            Update,
            connect_to_server
                .run_if(in_state(ConnectionState::Disconnected))
                .run_if(on_timer(Duration::from_secs_f64(1.0))),
        )
        .add_systems(Update, handle_network_events);
    }
}
