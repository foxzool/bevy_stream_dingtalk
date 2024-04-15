use std::time::Duration;

use bevy::prelude::*;
use bevy::tasks::TaskPoolBuilder;
use bevy::time::common_conditions::on_timer;
use bevy_eventwork::EventworkRuntime;
use bevy_eventwork_mod_websockets::{NetworkSettings, WebSocketProvider};

use crate::client::{ConnectionState, DingTalkClient};
use crate::system::*;

pub struct StreamDingTalkPlugin {
    pub client_id: String,
    pub client_secret: String,
}

impl Plugin for StreamDingTalkPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(bevy_eventwork::EventworkPlugin::<
            WebSocketProvider,
            bevy::tasks::TaskPool,
        >::default());
        app.insert_resource(NetworkSettings::default());
        app.insert_resource(EventworkRuntime(
            TaskPoolBuilder::new().num_threads(2).build(),
        ));
        debug!(
            "StreamDingTalkPlugin init with client_id: {}, client_secret: {}",
            self.client_id, self.client_secret
        );
        app.insert_resource(DingTalkClient::new(
            self.client_id.clone(),
            self.client_secret.clone(),
        ))
        .init_state::<ConnectionState>();
        app.add_systems(
            Update,
            connect_to_server
                .run_if(in_state(ConnectionState::Disconnected))
                .run_if(on_timer(Duration::from_secs_f64(1.0))),
        ).add_systems(Update, handle_network_events);
    }
}
