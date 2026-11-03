use anyhow::{Context, Result};
use http2::frame::PseudoId;
use http2::frame::SettingId;
use http2::frame::{Priority, StreamDependency};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
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

    #[serde(default)]
    pub response: Option<HeadersCfg>,

    #[serde(default)]
    pub reject: Option<RejectCfg>,

    #[serde(default)]
    pub reject_status: Option<u16>,

    #[serde(skip)]
    pub reject_body: Option<Vec<u8>>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct HeadersCfg {
    pub headers_order: Option<Vec<String>>,
    pub http_headers: Option<HashMap<String, Option<String>>>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct RejectCfg {
    pub headers_order: Option<Vec<String>>,
    pub http_headers: Option<HashMap<String, Option<String>>>,

    #[serde(default)]
    pub body_path: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct H2PriorityFrame {
    pub stream_id: u32,
    pub exclusive: bool,
    pub depends_on: u32,
    pub weight: u8,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct H2HeadersPriority {
    pub exclusive: bool,
    pub depends_on: u32,
    pub weight: u8,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub header_table_size: Option<u32>,
    pub enable_push: bool,
    pub max_concurrent_streams: Option<u32>,
    pub initial_window_size: Option<u32>,
    pub max_frame_size: Option<u32>,
    pub max_header_list_size: Option<u32>,
}

impl Http2Config {
    pub fn load(path: &str) -> Result<Self> {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read http2 profile '{path}'"))?;
        let mut config: Http2Config = toml::from_str(&contents)
            .with_context(|| format!("invalid http2 profile '{path}'"))?;
        if let Some(status) = config.reject_status {
            http::StatusCode::from_u16(status)
                .map_err(|e| anyhow::anyhow!("[H2] invalid reject_status {status}: {e}"))?;
        }
        if let Some(body_path) = config.reject.as_ref().and_then(|r| r.body_path.as_ref()) {
            config.reject_body = Some(
                std::fs::read(body_path)
                    .map_err(|e| anyhow::anyhow!("[H2] cannot read reject body '{body_path}': {e}"))?,
            );
        }
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        use anyhow::bail;
        const MAX_WINDOW: u32 = (1 << 31) - 1;
        if let Some(s) = &self.settings {
            if let Some(v) = s.max_frame_size {
                if !(16_384..=16_777_215).contains(&v) {
                    bail!("settings.max_frame_size = {v}: must be in 16384..=16777215");
                }
            }
            if let Some(v) = s.initial_window_size {
                if v > MAX_WINDOW {
                    bail!("settings.initial_window_size = {v}: must be <= {MAX_WINDOW}");
                }
            }
        }
        if let Some(v) = self.connection_window_update {
            if v > MAX_WINDOW {
                bail!("connection_window_update = {v}: must be <= {MAX_WINDOW}");
            }
        }
        if let Some(id) = self.initial_stream_id {
            if id % 2 == 0 {
                bail!("initial_stream_id = {id}: client stream ids must be odd");
            }
        }
        self.parse_pseudo_headers()?;
        self.parse_settings_order()?;
        self.apply_http_headers(&http::HeaderMap::new())?;
        self.apply_response_headers()?;
        self.apply_reject_headers()?;
        Ok(())
    }

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
        build_header_list(
            self.headers_order.as_ref(),
            self.http_headers.as_ref(),
            Some(incoming),
        )
    }

    pub fn apply_response_headers(
        &self,
    ) -> anyhow::Result<Vec<(http::header::HeaderName, http::header::HeaderValue)>> {
        match self.response.as_ref() {
            Some(h) => build_header_list(h.headers_order.as_ref(), h.http_headers.as_ref(), None),
            None => Ok(Vec::new()),
        }
    }

    pub fn apply_reject_headers(
        &self,
    ) -> anyhow::Result<Vec<(http::header::HeaderName, http::header::HeaderValue)>> {
        match self.reject.as_ref() {
            Some(h) => build_header_list(h.headers_order.as_ref(), h.http_headers.as_ref(), None),
            None => Ok(Vec::new()),
        }
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

fn build_header_list(
    order: Option<&Vec<String>>,
    map: Option<&HashMap<String, Option<String>>>,
    incoming: Option<&http::HeaderMap>,
) -> anyhow::Result<Vec<(http::header::HeaderName, http::header::HeaderValue)>> {
    use http::header::{HeaderName, HeaderValue};
    use std::collections::HashSet;

    let mut result = Vec::new();
    let mut processed = HashSet::new();

    if let Some(order) = order {
        for configured_name in order {
            let name_lower = configured_name.to_ascii_lowercase();

            let name = HeaderName::from_bytes(name_lower.as_bytes()).map_err(|err| {
                anyhow::anyhow!("[H2] invalid header name '{configured_name}': {err}")
            })?;

            processed.insert(name.clone());

            let configured_val = map.as_ref().and_then(|m| m.get(&name_lower));

            match configured_val {
                Some(Some(value)) => {
                    let value = HeaderValue::from_str(value).map_err(|err| {
                        anyhow::anyhow!("[H2] invalid value for header '{configured_name}': {err}")
                    })?;
                    result.push((name, value));
                }
                Some(None) => {}
                None => {
                    if let Some(incoming) = incoming {
                        for value in incoming.get_all(&name).iter() {
                            result.push((name.clone(), value.clone()));
                        }
                    }
                }
            }
        }
    }

    if let Some(map) = map {
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

    if let Some(incoming) = incoming {
        for (name, value) in incoming.iter() {
            if processed.contains(name) {
                continue;
            }
            result.push((name.clone(), value.clone()));
        }
    }

    Ok(result)
}
