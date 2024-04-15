use bevy::prelude::*;
use bevy::tasks::TaskPool;
use bevy_eventwork::{EventworkRuntime, Network, NetworkEvent};
use bevy_eventwork_mod_websockets::{NetworkSettings, WebSocketProvider};

use crate::client::{ConnectionState, DingTalkClient};

pub(crate) fn connect_to_server(
    net: ResMut<Network<WebSocketProvider>>,
    settings: Res<NetworkSettings>,
    task_pool: Res<EventworkRuntime<TaskPool>>,
    mut client: ResMut<DingTalkClient>,
    mut state: ResMut<NextState<ConnectionState>>
) {
    match client.get_endpoint() {
        Ok(url) => {
            net.connect(url::Url::parse(&url).unwrap(), &task_pool.0, &settings);
            state.set(ConnectionState::Connecting);
        }
        Err(err) => {
            error!("get endpoint error: {}", err);
            return;
        }
    }
    // net.connect(
    //     url::Url::parse("ws://127.0.0.1:8081").unwrap(),
    //     &task_pool.0,
    //     &settings,
    // );
}

pub(crate) fn handle_network_events(mut new_network_events: EventReader<NetworkEvent>, mut state: ResMut<NextState<ConnectionState>>) {
    for event in new_network_events.read() {
        info!("Received event {:?}", event);
        match event {
            NetworkEvent::Connected(_) => {
                state.set(ConnectionState::Connected);
            }
            NetworkEvent::Disconnected(_) => {
                state.set(ConnectionState::Disconnected);
            }
            NetworkEvent::Error(_) => {
                state.set(ConnectionState::Disconnected);
            }
        }
    }
}
