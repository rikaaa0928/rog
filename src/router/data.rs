use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::str::FromStr;

use crate::def::config;
use crate::router::consts::FORMAT_LAN;
use crate::router::matcher::{get_matcher_factory_fn, Matcher};
use reqwest::Url;
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
pub struct InnerRouteData {
    pub name: String,
    pub data: Vec<u8>,
    pub lines: Vec<String>,
    pub format: String,
}

pub async fn load_route_data(data_cfg: &[config::RouteData]) -> HashMap<String, Box<dyn Matcher>> {
    let mut data_map: HashMap<String, Box<dyn Matcher>> = HashMap::new();
    for rd_cfg in data_cfg {
        let rd_cfg = &rd_cfg;
        let mut rd = InnerRouteData {
            name: rd_cfg.name.clone(),
            data: Vec::new(),
            lines: Vec::new(),
            format: rd_cfg.format.clone(),
        };

        if rd.format == FORMAT_LAN {
            rd.lines = vec![
                "10.0.0.0/8".to_string(),
                "172.16.0.0/12".to_string(),
                "192.168.0.0/16".to_string(),
                "127.0.0.0/8".to_string(),
            ];
        } else if rd_cfg.url.is_some() {
            if let Err(e) =
                load_data_from_source(&mut rd, rd_cfg.url.as_ref().unwrap().as_str()).await
            {
                log::warn!(
                    "Error loading data for '{}' from '{}': {:?}",
                    rd_cfg.name,
                    rd_cfg.url.as_ref().unwrap(),
                    e
                );
                continue;
            }
        } else if rd_cfg.data.is_some() {
            rd.lines = rd_cfg
                .data
                .as_ref()
                .unwrap()
                .split("\n")
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.to_string())
                .collect();
        } else {
            log::warn!("No data source provided for route data '{}'", rd_cfg.name);
            continue;
        }
        if let Some(factory) = get_matcher_factory_fn(&rd.format) {
            data_map.insert(rd_cfg.name.clone(), factory(rd.lines, rd.data));
        }
    }
    data_map
}

async fn load_data_from_source(rd: &mut InnerRouteData, source_url: &str) -> Result<(), String> {
    let parsed_url =
        Url::from_str(source_url).map_err(|e| format!("invalid URL '{}': {}", source_url, e))?;

    match parsed_url.scheme() {
        "file" => {
            let path = parsed_url.path();
            let lines = load_route_data_from_file(path)
                .map_err(|e| format!("error loading from file '{}': {}", path, e))?;
            rd.lines = lines;
        }
        "http" | "https" => {
            let lines = load_route_data_from_url(source_url)
                .await
                .map_err(|e| format!("error loading from URL '{}': {}", source_url, e))?;
            rd.lines = lines;
        }
        _ => return Err(format!("unsupported URL scheme: {}", parsed_url.scheme())),
    }
    Ok(())
}

fn load_route_data_from_file(path: &str) -> io::Result<Vec<String>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut lines = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if !line.trim().is_empty() {
            lines.push(line);
        }
    }
    Ok(lines)
}

async fn load_route_data_from_url(url_str: &str) -> Result<Vec<String>, String> {
    let resp = reqwest::get(url_str)
        .await
        .map_err(|e| format!("failed to get URL '{}': {}", url_str, e))?;

    if resp.status() != reqwest::StatusCode::OK {
        return Err(format!(
            "failed to fetch URL '{}', status code: {}",
            url_str,
            resp.status()
        ));
    }

    let body = resp
        .text()
        .await
        .map_err(|e| format!("failed to read body from URL '{}': {}", url_str, e))?;

    let mut lines = Vec::new();
    for line in body.lines() {
        if !line.trim().is_empty() {
            lines.push(line.to_string());
        }
    }
    Ok(lines)
}
