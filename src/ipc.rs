use std::str::from_utf8;
use serde_json::Value;
use crate::message::Message;

use std::sync::Arc;
use async_std::sync::Mutex;

use async_std::os::unix::net::UnixStream;
use async_std::io::prelude::*;
use futures::stream::BoxStream;

use iced_futures::futures;
use iced_futures::subscription::Recipe;

async fn get_next_repait_loop_msg(socket: Arc<Mutex<UnixStream>>) -> Option<Value> {
    loop {
        let mut len_buf = [0; 4]; // Size is u32
        let mut s = socket.lock().await;
        if let Ok(_) = (*s).read_exact(&mut len_buf).await {
            let len = u32::from_ne_bytes(len_buf) as usize;

            let mut message_buf = vec![0u8; len];
            if let Ok(_) = (*s).read_exact(&mut message_buf).await {
                let msg_str = from_utf8(&message_buf).unwrap();
                let msg: Value = serde_json::from_str(msg_str).unwrap();

                if msg["category"] == "repaint-loop" {
                    return Some(msg);
                } else {
                    continue;
                }
            }
        }

        return None;
    }
}

pub struct WayfireSocketRecipe {
    socket: Arc<Mutex<UnixStream>>
}

impl WayfireSocketRecipe {
    pub fn new(socket: Arc<Mutex<UnixStream>>) -> Self {
        WayfireSocketRecipe { socket }
    }
}

impl<H, I> Recipe<H, I> for WayfireSocketRecipe
where H: std::hash::Hasher
{
    type Output = Message;
    fn hash(&self, state: &mut H) {
        use std::hash::Hash;

        // For now, only a single instance, so no need to hash anything
        std::any::TypeId::of::<Self>().hash(state);
    }

    fn stream(self: Box<Self>, _input: BoxStream<'static, I>) -> BoxStream<'static, Self::Output> {
        use futures::StreamExt;
        futures::stream::unfold(self, |wsocket| async {
            if let Some(msg) = get_next_repait_loop_msg(wsocket.socket.clone()).await {
                let time = msg["timestamp"].as_i64().unwrap() as u64;

                // Might be NONE
                let object = msg["object"].to_string();

                match msg["event"].as_str().unwrap() {
                    "start-paint" => Some((Message::FrameRepaint(object, time) , wsocket)),
                    "end-paint" => Some((Message::FrameRepaintDone(object, time) , wsocket)),
                    "start-frame" => Some((Message::FrameStart(object, time) , wsocket)),
                    "surface-commit" => {
                        let id = msg["object"].as_i64().unwrap() as u32;
                        let output = msg["output"].to_string();
                        Some((Message::SurfaceCommit(id, output, time), wsocket))
                    },
                    _ => panic!("Unknown event")
                }
            } else {
                // End of Stream, error, anything
                None
            }
        }).boxed()
    }
}
