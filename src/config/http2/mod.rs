use http2::frame::PseudoId;
use http2::frame::SettingId;
use http2::frame::{Priority, StreamDependency};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
pub struct Http2Config {
    pub settings: Option<Settings>,
    pub settings_order: Option<Vec<String>>,
    pub connection_window_update: Option<u32>,
    #[serde(default)]
    pub initial_stream_id: Option<u32>,
    #[serde(default)]
    pub priority_frames: Option<Vec<H2PriorityFrame>>,
    #[serde(default)]
    pub headers_priority: Option<H2HeadersPriority>,
    pub pseudo_headers_order: Option<Vec<String>>,
    pub headers_order: Option<Vec<String>>,
    pub http_headers: Option<HashMap<String, Option<String>>>,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
pub struct H2PriorityFrame {
    pub stream_id: u32,
    pub exclusive: bool,
    pub depends_on: u32,
    pub weight: u8,
}

#[derive(Debug, Deserialize, Clone)]
pub struct H2HeadersPriority {
    pub exclusive: bool,
    pub depends_on: u32,
    pub weight: u8,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Settings {
    pub header_table_size: Option<u32>,
    pub enable_push: bool,
    pub max_concurrent_streams: Option<u32>,
    pub initial_window_size: Option<u32>,
    pub max_frame_size: Option<u32>,
    pub max_header_list_size: Option<u32>,
}

impl Http2Config {
    pub fn parse_pseudo_headers(&self) -> anyhow::Result<Vec<PseudoId>> {
        self.pseudo_headers_order
            .as_ref()
            .map(|order| {
                order
                    .iter()
                    .map(|name| match name.as_str() {
                        ":method" => Ok(PseudoId::Method),
                        ":path" => Ok(PseudoId::Path),
                        ":authority" => Ok(PseudoId::Authority),
                        ":scheme" => Ok(PseudoId::Scheme),
                        _ => Err(anyhow::anyhow!("unknown pseudo header: {name}")),
                    })
                    .collect()
            })
            .unwrap_or_else(|| Ok(Vec::new()))
    }

    pub fn parse_settings_order(&self) -> anyhow::Result<Vec<SettingId>> {
        self.settings_order
            .as_ref()
            .map(|order| {
                order
                    .iter()
                    .map(|name| match name.to_ascii_lowercase().as_str() {
                        "header_table_size" => Ok(SettingId::HeaderTableSize),
                        "enable_push" => Ok(SettingId::EnablePush),
                        "max_concurrent_streams" => Ok(SettingId::MaxConcurrentStreams),
                        "initial_window_size" => Ok(SettingId::InitialWindowSize),
                        "max_frame_size" => Ok(SettingId::MaxFrameSize),
                        "max_header_list_size" => Ok(SettingId::MaxHeaderListSize),
                        _ => Err(anyhow::anyhow!("unknown setting: {name}")),
                    })
                    .collect()
            })
            .unwrap_or_else(|| Ok(Vec::new()))
    }

    pub fn parse_priority_frames(&self) -> Vec<Priority> {
        self.priority_frames
            .as_ref()
            .map(|frames| {
                frames
                    .iter()
                    .map(|frame| {
                        Priority::new(
                            frame.stream_id.into(),
                            StreamDependency::new(
                                frame.depends_on.into(),
                                frame.weight,
                                frame.exclusive,
                            ),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn parse_headers_priority(&self) -> Option<StreamDependency> {
        self.headers_priority
            .as_ref()
            .map(|p| StreamDependency::new(p.depends_on.into(), p.weight, p.exclusive))
    }

    pub fn apply_http_headers(
        &self,
        incoming: &http::HeaderMap,
    ) -> anyhow::Result<Vec<(http::header::HeaderName, http::header::HeaderValue)>> {
        use http::header::{HeaderName, HeaderValue};
        use std::collections::HashSet;

        let mut result = Vec::new();
        let mut processed = HashSet::new();

        if let Some(order) = &self.headers_order {
            for configured_name in order {
                let name_lower = configured_name.to_ascii_lowercase();

                let name = HeaderName::from_bytes(name_lower.as_bytes()).map_err(|err| {
                    anyhow::anyhow!("[H2] invalid header name '{configured_name}': {err}")
                })?;

                processed.insert(name.clone());

                let configured_val = self
                    .http_headers
                    .as_ref()
                    .and_then(|map| map.get(&name_lower));

                match configured_val {
                    Some(Some(value)) => {
                        let value = HeaderValue::from_str(value).map_err(|err| {
                            anyhow::anyhow!(
                                "[H2] invalid value for header '{configured_name}': {err}"
                            )
                        })?;
                        result.push((name, value));
                    }
                    Some(None) => {}
                    None => {
                        for value in incoming.get_all(&name).iter() {
                            result.push((name.clone(), value.clone()));
                        }
                    }
                }
            }
        }

        if let Some(map) = &self.http_headers {
            for (configured_name, configured_value) in map {
                let name_lower = configured_name.to_ascii_lowercase();

                let name = HeaderName::from_bytes(name_lower.as_bytes()).map_err(|err| {
                    anyhow::anyhow!("[H2] invalid header name '{configured_name}': {err}")
                })?;

                if processed.contains(&name) {
                    continue;
                }

                processed.insert(name.clone());

                if let Some(value) = configured_value {
                    let value = HeaderValue::from_str(value).map_err(|err| {
                        anyhow::anyhow!("[H2] invalid value for header '{configured_name}': {err}")
                    })?;
                    result.push((name, value));
                }
            }
        }

        for (name, value) in incoming.iter() {
            if processed.contains(name) {
                continue;
            }
            result.push((name.clone(), value.clone()));
        }

        Ok(result)
    }

    pub fn encode_alps_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        if let Some(settings_order) = &self.settings_order {
            if let Some(settings) = &self.settings {
                for setting_name in settings_order {
                    match setting_name.to_ascii_lowercase().as_str() {
                        "header_table_size" => {
                            if let Some(val) = settings.header_table_size {
                                bytes.extend_from_slice(&1u16.to_be_bytes());
                                bytes.extend_from_slice(&val.to_be_bytes());
                            }
                        }
                        "enable_push" => {
                            let val = if settings.enable_push { 1u32 } else { 0u32 };
                            bytes.extend_from_slice(&2u16.to_be_bytes());
                            bytes.extend_from_slice(&val.to_be_bytes());
                        }
                        "max_concurrent_streams" => {
                            if let Some(val) = settings.max_concurrent_streams {
                                bytes.extend_from_slice(&3u16.to_be_bytes());
                                bytes.extend_from_slice(&val.to_be_bytes());
                            }
                        }
                        "initial_window_size" => {
                            if let Some(val) = settings.initial_window_size {
                                bytes.extend_from_slice(&4u16.to_be_bytes());
                                bytes.extend_from_slice(&val.to_be_bytes());
                            }
                        }
                        "max_frame_size" => {
                            if let Some(val) = settings.max_frame_size {
                                bytes.extend_from_slice(&5u16.to_be_bytes());
                                bytes.extend_from_slice(&val.to_be_bytes());
                            }
                        }
                        "max_header_list_size" => {
                            if let Some(val) = settings.max_header_list_size {
                                bytes.extend_from_slice(&6u16.to_be_bytes());
                                bytes.extend_from_slice(&val.to_be_bytes());
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        bytes
    }
}
