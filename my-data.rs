use anyhow::Result;
use clap::{App, AppSettings, Arg};
use std::io::Write;
use std::sync::Arc;
use tokio::time::Duration;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::math_rand_alpha;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use firebase_rs::*;
extern crate serde_json;
use serde_json::{Value};
use serde::{Serialize, Deserialize};



#[derive(Serialize, Deserialize, Debug)]
struct Offer {
    description: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug)]
struct Answer {
    answer: String,
}


#[tokio::main]
async fn main() -> Result<()> {
    let app = App::new("datalink")
        .setting(AppSettings::DeriveDisplayOrder)
        .setting(AppSettings::SubcommandsNegateReqs)
        .arg(
            Arg::new("debug")
                .long("debug")
                .short('d')
                .help("Prints debug log information"),
        );

    let matches = app.clone().get_matches();

    let debug = matches.is_present("debug");
    if debug {
        env_logger::Builder::new()
            .format(|buf, record| {
                writeln!(
                    buf,
                    "{}:{} [{}] {} - {}",
                    record.file().unwrap_or("unknown"),
                    record.line().unwrap_or(0),
                    record.level(),
                    chrono::Local::now().format("%H:%M:%S.%6f"),
                    record.args()
                )
            })
            .filter(None, log::LevelFilter::Trace)
            .init();
    }

    let mut m = MediaEngine::default();

    m.register_default_codecs()?;

    let mut registry = Registry::new();

    registry = register_default_interceptors(registry, &mut m)?;

    let api = APIBuilder::new()
        .with_media_engine(m)
        .with_interceptor_registry(registry)
        .build();


        let config = RTCConfiguration {
        ice_servers: vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_owned()],
            ..Default::default()
        }],
        ..Default::default()
    };

    let peer_connection = Arc::new(api.new_peer_connection(config).await?);

    let (done_tx, mut done_rx) = tokio::sync::mpsc::channel::<()>(1);

    peer_connection
        .on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
            println!("Peer Connection State has changed: {}", s);

            if s == RTCPeerConnectionState::Failed {
                println!("Peer Connection has gone to failed exiting");
                let _ = done_tx.try_send(());
            }

            Box::pin(async {})
        }))
        .await;

        peer_connection
        .on_data_channel(Box::new(move |d: Arc<RTCDataChannel>| {
            let d_label = d.label().to_owned();
            let d_id = d.id();
            println!("New DataChannel {} {}", d_label, d_id);

            // Register channel opening handling
            Box::pin(async move {
                let d2 = Arc::clone(&d);
                let d_label2 = d_label.clone();
                let d_id2 = d_id;
                d.on_open(Box::new(move || {
                    println!("Data channel '{}'-'{}' open.", d_label2, d_id2);

                    Box::pin(async move {
                        let mut result = Result::<usize>::Ok(0);
                        while result.is_ok() {
                            let timeout = tokio::time::sleep(Duration::from_secs(5));
                            tokio::pin!(timeout);

                            tokio::select! {
                                _ = timeout.as_mut() =>{
                                    let message = math_rand_alpha(15);
                                    println!("Sending '{}'", message);
                                    result = d2.send_text(message).await.map_err(Into::into);
                                }
                            };
                        }
                    })
                })).await;

                // Register text message handling
                d.on_message(Box::new(move |msg: DataChannelMessage| {
                    let msg_str = String::from_utf8(msg.data.to_vec()).unwrap();
                    println!("'{}': '{}'", d_label, msg_str);
                    Box::pin(async {})
                })).await;
            })
        }))
        .await;

    let remote_descr = read_remote_description().await;
    println!(">>>REMOTE<<<: {}", remote_descr["offer"]);
    let offer = serde_json::from_str::<RTCSessionDescription>(&remote_descr["offer"].as_str().unwrap())?;
    println!(">>>OFFER<<<: {}", serde_json::to_string(&offer)?);
    peer_connection.set_remote_description(offer).await?;

    let answer = peer_connection.create_answer(None).await?;
    println!("<<<ANSWER>>>: {}", serde_json::to_string(&answer)?);
    let mut gather_complete = peer_connection.gathering_complete_promise().await;

    peer_connection.set_local_description(answer).await?;

    let _ = gather_complete.recv().await;

    if let Some(local_descr) = peer_connection.local_description().await {
        let json_local = serde_json::to_string(&local_descr)?;
        println!("<<<LOCAL>>>: {}", json_local);
        set_local_description(json_local).await;
    } else {
        println!("generate local_description failed!");
    }

    println!("Press ctrl-c to stop");
    tokio::select! {
        _ = done_rx.recv() => {
            println!("received done signal!");
        }
        _ = tokio::signal::ctrl_c() => {
            println!("");
        }
    };

    peer_connection.close().await?;

    Ok(())
}

async fn set_local_description(description: String) {
    let ans = Answer {answer: description};
    let firebase = Firebase::new("https://artidatachannel-default-rtdb.europe-west1.firebasedatabase.app").unwrap().at("negotiations").at("01");
    let _ = firebase.update(&ans).await;
}

async fn read_remote_description() -> Value {
    let firebase = Firebase::new("https://artidatachannel-default-rtdb.europe-west1.firebasedatabase.app").unwrap().at("negotiations").at("01");
    let data = firebase.get::<Value>().await;
    let d = data.unwrap();
    d
}