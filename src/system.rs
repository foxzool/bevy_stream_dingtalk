use bevy::prelude::*;
use bevy::tasks::TaskPool;


use crate::client::{ConnectionState, Client, DingTalkClient, AsyncRuntime};
use crate::client::down::RobotRecvMessage;
use crate::client::up::EventAckData;
use crate::constant::TOPIC_ROBOT;

pub(crate) fn connect_to_server(
    mut client: ResMut<DingTalkClient>,
    rt: Res<AsyncRuntime>,
    mut state: ResMut<NextState<ConnectionState>>,
) {

    let client = client.clone();
    rt.spawn(async {
        client
            .register_callback_listener(TOPIC_ROBOT, |client, msg| {
                async move {
                    let RobotRecvMessage {
                        content,
                        sender_staff_id,
                        conversation_id,
                        conversation_type,
                        sender_nick,
                        ..
                    } = msg;
                    println!("Message Received from {}: {:?}", sender_nick, content);

                    Ok::<_, anyhow::Error>(())
                }
            })
            .register_all_event_listener(|msg| {
                println!("event: {:?}", msg);
                EventAckData::default()
            })
            .connect().await.unwrap();
    });

    state.set(ConnectionState::Connecting);

    // match client.get_endpoint() {
    //     Ok(url) => {
    //         net.connect(url::Url::parse(&url).unwrap(), &task_pool.0, &settings);
    //         state.set(ConnectionState::Connecting);
    //     }
    //     Err(err) => {
    //         error!("get endpoint error: {}", err);
    //         return;
    //     }
    // }
    // net.connect(
    //     url::Url::parse("ws://127.0.0.1:8081").unwrap(),
    //     &task_pool.0,
    //     &settings,
    // );
}

pub(crate) fn handle_network_events(

) {

}