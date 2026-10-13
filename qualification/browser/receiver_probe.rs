#[wasm_bindgen::prelude::wasm_bindgen]
pub async fn receiver_bounds_probe(url: String) -> Result<String, wasm_bindgen::JsValue> {
    use futures_util::StreamExt;
    let (_meta, mut stream) = ws_stream_wasm::WsMeta::connect(url, None)
        .await
        .map_err(|_| wasm_bindgen::JsValue::from_str("websocket_connect_failed"))?;
    n0_future::time::sleep(std::time::Duration::from_millis(150)).await;
    let received = n0_future::time::timeout(std::time::Duration::from_secs(3), stream.next())
        .await
        .map_err(|_| wasm_bindgen::JsValue::from_str("websocket_close_timeout"))?;
    Ok(if received.is_none() {
        "closed"
    } else {
        "unbounded_message"
    }
    .into())
}
