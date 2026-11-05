use crate::profiles::{Xhttp2Config, get_xhttp2};
use anyhow::Result;
use http2::frame::{Priorities, PseudoOrder, SettingsOrder};

pub fn xhttp2_builder(path: &str) -> Result<http2::client::Builder> {
    let config = get_xhttp2(path)?;
    validate(config.as_ref())?;

    let mut builder = http2::client::Builder::new();

    let mut pseudo_builder = PseudoOrder::builder();
    let pseudo_headers_order = config.parse_pseudo_headers()?;
    for id in pseudo_headers_order {
        pseudo_builder = pseudo_builder.push(id);
    }
    builder.headers_pseudo_order(pseudo_builder.build());

    let mut settings_builder = SettingsOrder::builder();
    let settings_order = config.parse_settings_order()?;
    for id in settings_order {
        settings_builder = settings_builder.push(id);
    }
    builder.settings_order(settings_builder.build());

    set_settings_frame(&mut builder, &config)?;

    if let Some(frames) = &config.priority_frames
        && !frames.is_empty()
    {
        let mut priorities_builder = Priorities::builder();
        for frame in config.parse_priority_frames() {
            priorities_builder = priorities_builder.push(frame);
        }
        builder.priorities(priorities_builder.build());
    }

    if let Some(dependency) = config.parse_headers_priority() {
        builder.headers_stream_dependency(dependency);
    }

    if let Some(cwu) = config.connection_window_update {
        builder.initial_connection_window_size(cwu);
    }

    let initial_id = if let Some(explicit) = config.initial_stream_id {
        explicit
    } else {
        match &config.priority_frames {
            Some(frames) if !frames.is_empty() => {
                frames.iter().map(|f| f.stream_id).max().unwrap_or(0) + 2
            }
            _ => 1,
        }
    };
    builder.initial_stream_id(initial_id);

    Ok(builder)
}

pub fn set_settings_frame(
    builder: &mut http2::client::Builder,
    config: &Xhttp2Config,
) -> Result<()> {
    if let Some(settings) = &config.settings {
        if let Some(v) = settings.header_table_size {
            builder.header_table_size(v);
        }

        if let Some(v) = settings.initial_window_size {
            builder.initial_window_size(v);
        }

        if let Some(v) = settings.max_frame_size {
            builder.max_frame_size(v);
        }

        if let Some(v) = settings.max_concurrent_streams {
            builder.max_concurrent_streams(v);
        }

        builder.enable_push(settings.enable_push);

        if let Some(v) = settings.max_header_list_size {
            builder.max_header_list_size(v);
        }
    }

    Ok(())
}

fn validate(x: &Xhttp2Config) -> Result<()> {
    const MAX_WINDOW: u32 = (1 << 31) - 1;
    if x.settings.as_ref().is_some() {
        if let Some(v) = x.settings.as_ref().unwrap().max_frame_size
            && !(16_384..=16_777_215).contains(&v)
        {
            anyhow::bail!("settings.max_frame_size = {v}: must be in 16384..=16777215");
        }
        if let Some(v) = x.settings.as_ref().unwrap().initial_window_size
            && v > MAX_WINDOW
        {
            anyhow::bail!("settings.initial_window_size = {v}: must be <= {MAX_WINDOW}");
        }
    }
    if let Some(v) = x.connection_window_update
        && v > MAX_WINDOW
    {
        anyhow::bail!("connection_window_update = {v}: must be <= {MAX_WINDOW}");
    }
    if let Some(id) = x.initial_stream_id
        && id % 2 == 0
    {
        anyhow::bail!("initial_stream_id = {id}: client stream ids must be odd");
    }
    x.parse_pseudo_headers()?;
    x.parse_settings_order()?;
    x.apply_http_headers(&http::Method::GET, &http::HeaderMap::new())?;
    x.apply_http_headers(&http::Method::POST, &http::HeaderMap::new())?;
    Ok(())
}
