//! websocket connection for chain rpc
//!
//! optimized for low-latency:
//! - binary protocol (no json overhead)
//! - non-blocking message queue
//! - automatic reconnection

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{WebSocket, MessageEvent, CloseEvent, ErrorEvent, BinaryType};
use std::cell::RefCell;
use std::rc::Rc;
use std::collections::VecDeque;

/// websocket connection error
#[derive(Debug)]
pub struct WsError(pub String);

impl std::fmt::Display for WsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// websocket connection wrapper
pub struct WsConnection {
    ws: WebSocket,
    /// incoming message queue (non-blocking)
    recv_queue: Rc<RefCell<VecDeque<Vec<u8>>>>,
    /// connection state
    connected: Rc<RefCell<bool>>,
    /// error message
    error: Rc<RefCell<Option<String>>>,
}

impl WsConnection {
    /// connect to endpoint
    pub async fn connect(endpoint: &str) -> Result<Self, WsError> {
        let ws = WebSocket::new(endpoint)
            .map_err(|e| WsError(format!("ws create failed: {:?}", e)))?;

        // use binary mode for efficiency
        ws.set_binary_type(BinaryType::Arraybuffer);

        let recv_queue = Rc::new(RefCell::new(VecDeque::with_capacity(64)));
        let connected = Rc::new(RefCell::new(false));
        let error = Rc::new(RefCell::new(None));

        // set up callbacks
        {
            let connected_clone = connected.clone();
            let onopen = Closure::wrap(Box::new(move |_: JsValue| {
                *connected_clone.borrow_mut() = true;
            }) as Box<dyn FnMut(JsValue)>);
            ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));
            onopen.forget();
        }

        {
            let recv_queue_clone = recv_queue.clone();
            let onmessage = Closure::wrap(Box::new(move |e: MessageEvent| {
                if let Ok(buffer) = e.data().dyn_into::<js_sys::ArrayBuffer>() {
                    let array = js_sys::Uint8Array::new(&buffer);
                    let data = array.to_vec();
                    recv_queue_clone.borrow_mut().push_back(data);
                }
            }) as Box<dyn FnMut(MessageEvent)>);
            ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
            onmessage.forget();
        }

        {
            let connected_clone = connected.clone();
            let onclose = Closure::wrap(Box::new(move |_: CloseEvent| {
                *connected_clone.borrow_mut() = false;
            }) as Box<dyn FnMut(CloseEvent)>);
            ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));
            onclose.forget();
        }

        {
            let error_clone = error.clone();
            let onerror = Closure::wrap(Box::new(move |e: ErrorEvent| {
                *error_clone.borrow_mut() = Some(e.message());
            }) as Box<dyn FnMut(ErrorEvent)>);
            ws.set_onerror(Some(onerror.as_ref().unchecked_ref()));
            onerror.forget();
        }

        // wait for connection (with timeout)
        let start = crate::now();
        while !*connected.borrow() {
            if crate::now() - start > 5000.0 {
                return Err(WsError("connection timeout".into()));
            }
            if let Some(err) = error.borrow().as_ref() {
                return Err(WsError(err.clone()));
            }
            // yield to js event loop
            wasm_bindgen_futures::JsFuture::from(
                js_sys::Promise::new(&mut |resolve, _| {
                    let _ = web_sys::window()
                        .unwrap()
                        .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 10);
                })
            ).await.ok();
        }

        Ok(Self {
            ws,
            recv_queue,
            connected,
            error,
        })
    }

    /// check if connected
    pub fn is_connected(&self) -> bool {
        *self.connected.borrow()
    }

    /// close connection
    pub fn close(&self) {
        let _ = self.ws.close();
    }

    /// send raw bytes
    pub fn send(&self, data: &[u8]) -> Result<(), WsError> {
        self.ws.send_with_u8_array(data)
            .map_err(|e| WsError(format!("send failed: {:?}", e)))
    }

    /// try to receive message (non-blocking)
    pub fn try_recv(&self) -> Option<Vec<u8>> {
        self.recv_queue.borrow_mut().pop_front()
    }

    /// subscribe to channel updates
    pub async fn subscribe_channel(&self, channel_id: &[u8; 32]) -> Result<(), WsError> {
        // encode subscribe request
        // format: [0x10][channel_id: 32]
        let mut msg = vec![0x10];
        msg.extend_from_slice(channel_id);
        self.send(&msg)
    }

    /// query balance
    pub async fn query_balance(&self, account: &[u8; 32]) -> Result<u64, WsError> {
        // encode balance query
        // format: [0x20][account: 32]
        let mut msg = vec![0x20];
        msg.extend_from_slice(account);
        self.send(&msg)?;

        // wait for response
        let start = crate::now();
        loop {
            if crate::now() - start > 5000.0 {
                return Err(WsError("query timeout".into()));
            }

            if let Some(resp) = self.try_recv() {
                // response format: [0x21][balance: 8]
                if resp.len() >= 9 && resp[0] == 0x21 {
                    let balance = u64::from_le_bytes(resp[1..9].try_into().unwrap());
                    return Ok(balance);
                }
            }

            // yield
            wasm_bindgen_futures::JsFuture::from(
                js_sys::Promise::new(&mut |resolve, _| {
                    let _ = web_sys::window()
                        .unwrap()
                        .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 5);
                })
            ).await.ok();
        }
    }

    /// submit transaction
    pub async fn submit_tx(&self, tx_bytes: &[u8]) -> Result<(), WsError> {
        // encode tx submit
        // format: [0x30][len: 4][tx_bytes]
        let mut msg = vec![0x30];
        msg.extend_from_slice(&(tx_bytes.len() as u32).to_le_bytes());
        msg.extend_from_slice(tx_bytes);
        self.send(&msg)
    }
}

impl Drop for WsConnection {
    fn drop(&mut self) {
        let _ = self.ws.close();
    }
}
