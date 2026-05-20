use serde_json::Value;

pub fn packet_payload(wrapper: &Value) -> &Value {
    wrapper
        .get("packet")
        .and_then(|packet| packet.get("payload"))
        .expect("packet.payload should exist")
}
