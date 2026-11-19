//! Model Context Protocol server over stdio.
//!
//! `arsenal mcp` speaks newline-delimited JSON-RPC 2.0 on stdin/stdout. It
//! exposes the configured asset roots as MCP tools and resources so an agent
//! can inspect, create, edit, install, and remove assets without the TUI.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use fuzzy_matcher::skim::SkimMatcherV2;
use serde_json::{Value, json};
use walkdir::WalkDir;

use crate::asset::{self, Asset, Kind};
use crate::config::{self, Config, Source};
use crate::install;
use crate::search::{self, SortMode};

const LATEST_PROTOCOL: &str = "2025-11-25";
const SUPPORTED_PROTOCOLS: [&str; 4] = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];
const MAX_LIST: usize = 1000;
const MAX_SKILL_FILES: usize = 200;

struct RpcError {
    code: i64,
    message: String,
}

impl RpcError {
    fn invalid_params(message: impl Into<String>) -> Self {
        RpcError {
            code: -32602,
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        RpcError {
            code: -32002,
            message: message.into(),
        }
    }
}

type RpcResult = std::result::Result<Value, RpcError>;

pub struct Server {
    cfg: Config,
    config_path: PathBuf,
}

impl Server {
    pub fn new(cfg: Config, config_path: PathBuf) -> Self {
        Server { cfg, config_path }
    }

    /// Handle one JSON-RPC message. Notifications return `None`.
    pub fn handle(&mut self, request: &Value) -> Option<Value> {
        let id = request.get("id").cloned()?;
        let Some(method) = request.get("method").and_then(Value::as_str) else {
            return Some(rpc_error(
                id,
                -32600,
                "Invalid Request: missing method".into(),
            ));
        };
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));

        let outcome = match method {
            "initialize" => Ok(self.initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => self.call_tool(&params),
            "resources/list" => self.list_resources(),
            "resources/templates/list" => Ok(json!({ "resourceTemplates": [] })),
            "resources/read" => self.read_resource(&params),
            "logging/setLevel" => Ok(json!({})),
            "completion/complete" => Ok(json!({ "completion": { "values": [] } })),
            _ => Err(RpcError {
                code: -32601,
                message: format!("Method not found: {method}"),
            }),
        };

        Some(match outcome {
            Ok(result) => rpc_result(id, result),
            Err(error) => rpc_error(id, error.code, error.message),
        })
    }

    fn initialize(&self, params: &Value) -> Value {
        let requested = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(LATEST_PROTOCOL);
        let protocol = if SUPPORTED_PROTOCOLS.contains(&requested) {
            requested
        } else {
            LATEST_PROTOCOL
        };
        json!({
            "protocolVersion": protocol,
            "capabilities": {
                "tools": { "listChanged": false },
                "resources": { "subscribe": false, "listChanged": false }
            },
            "serverInfo": {
                "name": "arsenal",
                "title": "Arsenal",
                "version": env!("CARGO_PKG_VERSION")
            },
            "instructions": format!(
                "Arsenal manages Markdown skills, agents, commands, rules, and snippets in configured source roots. \
                 Use list_assets to discover assets and read_asset to inspect one. Use create_asset, write_asset, \
                 install_asset, and delete_asset to change them. Writes are restricted to configured sources; \
                 deletes move assets to the trash unless purge is true. Configuration: {}",
                self.config_path.display()
            )
        })
    }

    fn call_tool(&mut self, params: &Value) -> RpcResult {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::invalid_params("`name` is required"))?;
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if !arguments.is_object() {
            return Err(RpcError::invalid_params("`arguments` must be an object"));
        }

        let outcome = match name {
            "list_assets" => self.list_assets(&arguments),
            "read_asset" => self.read_asset(&arguments),
            "create_asset" => self.create_asset(&arguments),
            "write_asset" => self.write_asset(&arguments),
            "delete_asset" => self.delete_asset(&arguments),
            "install_asset" => self.install_asset(&arguments),
            "list_sources" => self.list_sources(),
            "server_health" => Ok(health_value(&self.cfg, &self.config_path).0),
            _ => Err(format!("unknown tool `{name}`; call tools/list")),
        };

        Ok(match outcome {
            Ok(value) => tool_text(value, false),
            Err(message) => tool_text(Value::String(message), true),
        })
    }

    fn scan(&self) -> Vec<Asset> {
        let report = asset::scan(&self.cfg.sources, &self.cfg.behavior);
        for warning in report.warnings {
            eprintln!("arsenal mcp: {warning}");
        }
        report.assets
    }

    fn list_assets(&self, args: &Value) -> std::result::Result<Value, String> {
        let kind = optional_kind(args)?;
        let query = optional_str(args, "query")?.unwrap_or_default();
        let content = optional_bool(args, "content")?.unwrap_or(self.cfg.search.content);
        let limit = optional_usize(args, "limit")?
            .unwrap_or(100)
            .clamp(1, MAX_LIST);
        let assets = self.scan();
        let matcher = SkimMatcherV2::default();
        let ids = search::select(&assets, kind, &query, SortMode::Kind, content, &matcher);
        let total = ids.len();
        let items: Vec<Value> = ids
            .into_iter()
            .take(limit)
            .map(|index| asset_summary(&assets[index]))
            .collect();
        let returned = items.len();
        Ok(json!({
            "total": total,
            "returned": returned,
            "truncated": returned < total,
            "assets": items
        }))
    }

    fn read_asset(&self, args: &Value) -> std::result::Result<Value, String> {
        let assets = self.scan();
        let asset = resolve_asset(&assets, args)?;
        let content = fs::read_to_string(&asset.path)
            .map_err(|error| format!("reading {}: {error}", asset.path.display()))?;
        let mut result = json!({
            "asset": asset_summary(asset),
            "content": content
        });
        if asset.kind == Kind::Skill
            && let Some(directory) = asset.path.parent()
        {
            result["files"] = Value::Array(skill_files(directory));
        }
        Ok(result)
    }

    fn create_asset(&self, args: &Value) -> std::result::Result<Value, String> {
        let kind = required_kind(args)?;
        let name = required_str(args, "name")?;
        asset::validate_name(&name)?;
        let description = optional_str(args, "description")?.unwrap_or_default();
        let source = self.choose_source(kind, optional_str(args, "dest")?)?;
        let path = asset::asset_path(&source, kind, &name);
        let existed = path.exists();
        if existed && !optional_bool(args, "overwrite")?.unwrap_or(false) {
            return Err(format!(
                "{} already exists; pass overwrite: true to replace it",
                path.display()
            ));
        }
        let content = match args.get("content") {
            None | Some(Value::Null) => None,
            Some(Value::String(text)) => Some(text.clone()),
            Some(_) => return Err("`content` must be a string".into()),
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("creating {}: {error}", parent.display()))?;
        }
        let document =
            content.unwrap_or_else(|| asset::template(kind, &asset::slugify(&name), &description));
        asset::write_atomic(&path, &document)
            .map_err(|error| format!("writing {}: {error}", path.display()))?;
        let actual_name = Asset::load(&path, kind, &source.path)
            .map(|asset| asset.name)
            .unwrap_or(name);
        Ok(json!({
            "created": !existed,
            "overwritten": existed,
            "kind": kind.as_str(),
            "name": actual_name,
            "path": path.display().to_string()
        }))
    }

    fn write_asset(&self, args: &Value) -> std::result::Result<Value, String> {
        let content = required_text(args, "content")?;
        let assets = self.scan();
        let asset = resolve_asset(&assets, args)?;
        asset::write_atomic(&asset.path, &content)
            .map_err(|error| format!("writing {}: {error}", asset.path.display()))?;
        Ok(json!({
            "updated": true,
            "kind": asset.kind.as_str(),
            "name": asset.name,
            "path": asset.path.display().to_string(),
            "bytes": content.len()
        }))
    }

    fn delete_asset(&self, args: &Value) -> std::result::Result<Value, String> {
        let assets = self.scan();
        let asset = resolve_asset(&assets, args)?;
        let purge = optional_bool(args, "purge")?.unwrap_or(false);
        let target = removal_target(asset);
        let trashed = if purge || !self.cfg.behavior.trash {
            if target.is_dir() {
                fs::remove_dir_all(&target)
            } else {
                fs::remove_file(&target)
            }
            .map_err(|error| format!("removing {}: {error}", target.display()))?;
            None
        } else {
            Some(
                asset::trash_path(&target, &config::data_dir())
                    .map_err(|error| format!("trashing {}: {error}", target.display()))?,
            )
        };
        Ok(json!({
            "deleted": true,
            "kind": asset.kind.as_str(),
            "name": asset.name,
            "path": target.display().to_string(),
            "trash": trashed.map(|path| path.display().to_string())
        }))
    }

    fn install_asset(&self, args: &Value) -> std::result::Result<Value, String> {
        let url = required_str(args, "url")?;
        let kind = optional_kind(args)?.unwrap_or(Kind::Skill);
        let source = match optional_str(args, "dest")? {
            Some(dest) => Some(self.choose_source(kind, Some(dest))?),
            None => None,
        };
        let dest = match source {
            Some(source) => source.resolved(),
            None => self
                .cfg
                .install_source(kind)
                .map(Source::resolved)
                .ok_or_else(|| {
                    format!(
                        "no configured {} source; add one with `arsenal sources add` or pass dest",
                        kind.plural()
                    )
                })?,
        };
        let request = install::Request {
            url,
            kind,
            dest,
            name: optional_str(args, "name")?,
            reference: optional_str(args, "ref")?,
            subdir: optional_str(args, "subdir")?,
            force: optional_bool(args, "force")?.unwrap_or(false),
            trash_dir: None,
        };
        let installed = install::install(&request).map_err(|error| format!("{error:#}"))?;
        Ok(json!({
            "installed": true,
            "kind": installed.kind.as_str(),
            "name": installed.name,
            "path": installed.path.display().to_string(),
            "files": installed.files
        }))
    }

    fn list_sources(&self) -> std::result::Result<Value, String> {
        let sources: Vec<Value> = self
            .cfg
            .sources
            .iter()
            .map(|source| {
                json!({
                    "kind": source.kind.as_str(),
                    "path": source.path,
                    "resolved": source.resolved().display().to_string(),
                    "label": source.label,
                    "exists": source.resolved().is_dir()
                })
            })
            .collect();
        Ok(json!({ "sources": sources }))
    }

    fn list_resources(&self) -> RpcResult {
        let assets = self.scan();
        let resources: Vec<Value> = assets
            .iter()
            .map(|asset| {
                json!({
                    "uri": file_uri(&asset.path),
                    "name": format!("{}: {}", asset.kind.as_str(), asset.name),
                    "description": asset.description,
                    "mimeType": "text/markdown",
                    "size": asset.size
                })
            })
            .collect();
        Ok(json!({ "resources": resources }))
    }

    fn read_resource(&self, params: &Value) -> RpcResult {
        let uri = params
            .get("uri")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::invalid_params("`uri` is required"))?;
        let path = path_from_file_uri(uri).ok_or_else(|| {
            RpcError::invalid_params("`uri` must be a file:// URL returned by resources/list")
        })?;
        let target = canonical(&path);
        let assets = self.scan();
        let asset = assets
            .iter()
            .find(|asset| canonical(&asset.path) == target)
            .ok_or_else(|| RpcError::not_found(format!("unknown resource `{uri}`")))?;
        let text = fs::read_to_string(&asset.path).map_err(|error| RpcError {
            code: -32603,
            message: format!("reading {}: {error}", asset.path.display()),
        })?;
        Ok(json!({
            "contents": [{
                "uri": uri,
                "mimeType": "text/markdown",
                "text": text
            }]
        }))
    }

    fn choose_source(
        &self,
        kind: Kind,
        dest: Option<String>,
    ) -> std::result::Result<Source, String> {
        if let Some(dest) = dest {
            let target = canonical(&config::expand_path(&dest));
            return self
                .cfg
                .sources
                .iter()
                .find(|source| source.kind == kind && canonical(&source.resolved()) == target)
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "`{dest}` is not a configured {} source; call list_sources",
                        kind.as_str()
                    )
                });
        }
        self.cfg.install_source(kind).cloned().ok_or_else(|| {
            format!(
                "no configured {} source; add one with `arsenal sources add` or pass dest",
                kind.plural()
            )
        })
    }
}

pub fn serve(cfg: Config, config_path: PathBuf) -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    let mut server = Server::new(cfg, config_path);
    for line in stdin.lock().lines() {
        let line = line.context("reading MCP request from stdin")?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(request) => server.handle(&request),
            Err(error) => Some(rpc_error(
                Value::Null,
                -32700,
                format!("Parse error: {error}"),
            )),
        };
        if let Some(response) = response {
            write_message(&mut stdout, &response)?;
        }
    }
    Ok(())
}

/// Run a one-shot preflight check without starting the server. Returns whether
/// the configuration is healthy; the caller decides the process exit code.
pub fn health(cfg: &Config, config_path: &Path, json: bool) -> Result<bool> {
    let (value, healthy) = health_value(cfg, config_path);
    if json {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!(
            "status:    {}",
            value["status"].as_str().unwrap_or("unknown")
        );
        println!("version:   {}", value["version"].as_str().unwrap_or("?"));
        println!("protocol:  {}", value["protocol"].as_str().unwrap_or("?"));
        println!(
            "config:    {}",
            value["config"].as_str().unwrap_or("unknown")
        );
        println!(
            "sources:   {}/{} exist",
            value["sources_active"], value["sources_total"]
        );
        println!(
            "assets:    {} ({})",
            value["asset_total"],
            asset_count_summary(&value["assets"])
        );
        for warning in value["warnings"].as_array().into_iter().flatten() {
            if let Some(warning) = warning.as_str() {
                println!("warning:   {warning}");
            }
        }
    }
    io::stdout().flush().context("writing health report")?;
    Ok(healthy)
}

fn health_value(cfg: &Config, config_path: &Path) -> (Value, bool) {
    let report = asset::scan(&cfg.sources, &cfg.behavior);
    let status = if report.active_sources == 0 {
        "unhealthy"
    } else if report.warnings.is_empty() {
        "ok"
    } else {
        "degraded"
    };
    let healthy = status != "unhealthy";

    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for asset in &report.assets {
        *counts.entry(asset.kind.as_str()).or_default() += 1;
    }

    let sources: Vec<Value> = cfg
        .sources
        .iter()
        .map(|source| {
            json!({
                "kind": source.kind.as_str(),
                "path": source.path,
                "resolved": source.resolved().display().to_string(),
                "label": source.label,
                "exists": source.resolved().is_dir(),
                "assets": report
                    .assets
                    .iter()
                    .filter(|asset| asset.source == source.path)
                    .count()
            })
        })
        .collect();

    let value = json!({
        "status": status,
        "healthy": healthy,
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": LATEST_PROTOCOL,
        "config": config_path.display().to_string(),
        "sources_active": report.active_sources,
        "sources_total": cfg.sources.len(),
        "sources": sources,
        "asset_total": report.assets.len(),
        "assets": {
            "skill": counts.get("skill").copied().unwrap_or(0),
            "agent": counts.get("agent").copied().unwrap_or(0),
            "command": counts.get("command").copied().unwrap_or(0),
            "rule": counts.get("rule").copied().unwrap_or(0),
            "snippet": counts.get("snippet").copied().unwrap_or(0)
        },
        "warnings": report.warnings
    });
    (value, healthy)
}

fn asset_count_summary(counts: &Value) -> String {
    let summary = ["skill", "agent", "command", "rule", "snippet"]
        .into_iter()
        .filter_map(|kind| {
            let count = counts[kind].as_u64().unwrap_or(0);
            (count > 0).then(|| format!("{count} {kind}"))
        })
        .collect::<Vec<_>>()
        .join(", ");
    if summary.is_empty() {
        "none".into()
    } else {
        summary
    }
}

fn write_message(out: &mut impl Write, message: &Value) -> Result<()> {
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    out.write_all(&line)?;
    out.flush()?;
    Ok(())
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: Value, code: i64, message: String) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

fn tool_text(value: Value, is_error: bool) -> Value {
    let text = match value {
        Value::String(text) => text,
        value => serde_json::to_string_pretty(&value).unwrap_or_default(),
    };
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error
    })
}

fn asset_summary(asset: &Asset) -> Value {
    json!({
        "kind": asset.kind.as_str(),
        "name": asset.name,
        "description": asset.description,
        "tags": asset.tags,
        "path": asset.path.display().to_string(),
        "source": asset.source,
        "modified": asset
            .modified
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs()),
        "size": asset.size
    })
}

fn tools() -> Vec<Value> {
    let kind = json!({
        "type": "string",
        "enum": ["skill", "agent", "command", "rule", "snippet"]
    });
    let asset_ref = json!({
        "path": {
            "type": "string",
            "description": "Asset file path, or a skill directory"
        },
        "kind": kind,
        "name": {
            "type": "string",
            "description": "Asset name from frontmatter or file stem"
        }
    });

    vec![
        json!({
            "name": "list_assets",
            "title": "List assets",
            "description": "List skills, agents, commands, rules, and snippets in the configured source roots. Supports the same fuzzy query and #tag filters as the TUI.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "kind": kind,
                    "query": {
                        "type": "string",
                        "description": "Fuzzy words matched against name, kind, tags, and description; #tag filters tags"
                    },
                    "content": {
                        "type": "boolean",
                        "description": "Also match document bodies"
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_LIST,
                        "default": 100
                    }
                },
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "read_asset",
            "title": "Read asset",
            "description": "Read one asset's Markdown and metadata. For skills, also returns the files in the skill directory.",
            "inputSchema": {
                "type": "object",
                "properties": asset_ref,
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "create_asset",
            "title": "Create asset",
            "description": "Create a new asset in a configured source root. Without content, writes Arsenal's scaffolding template.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "kind": kind,
                    "name": { "type": "string" },
                    "description": { "type": "string" },
                    "content": {
                        "type": "string",
                        "description": "Complete Markdown document including frontmatter"
                    },
                    "dest": {
                        "type": "string",
                        "description": "Configured source root; defaults to the OMP root or the first source for the kind"
                    },
                    "overwrite": {
                        "type": "boolean",
                        "default": false
                    }
                },
                "required": ["kind", "name"],
                "additionalProperties": false
            },
            "annotations": { "destructiveHint": false, "idempotentHint": false }
        }),
        json!({
            "name": "write_asset",
            "title": "Write asset",
            "description": "Replace an existing asset's complete Markdown document. Read it first unless you intend a full rewrite; writes are limited to assets found in configured sources.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Asset file path, or a skill directory"
                    },
                    "kind": kind,
                    "name": { "type": "string" },
                    "content": {
                        "type": "string",
                        "description": "Complete Markdown document including frontmatter"
                    }
                },
                "required": ["content"],
                "additionalProperties": false
            },
            "annotations": { "destructiveHint": true, "idempotentHint": true }
        }),
        json!({
            "name": "delete_asset",
            "title": "Delete asset",
            "description": "Remove an asset. Skills are removed as whole directories. By default the asset moves to Arsenal's trash; purge deletes permanently.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Asset file path, or a skill directory"
                    },
                    "kind": kind,
                    "name": { "type": "string" },
                    "purge": {
                        "type": "boolean",
                        "default": false
                    }
                },
                "additionalProperties": false
            },
            "annotations": { "destructiveHint": true, "idempotentHint": false }
        }),
        json!({
            "name": "install_asset",
            "title": "Install asset",
            "description": "Clone a GitHub/git asset into a configured source root, matching `arsenal install`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "GitHub tree/blob/repository URL, owner/repo shorthand, or git URL"
                    },
                    "kind": kind,
                    "dest": {
                        "type": "string",
                        "description": "Configured source root; defaults to the OMP root or the first source for the kind"
                    },
                    "name": { "type": "string" },
                    "ref": {
                        "type": "string",
                        "description": "Git branch, tag, or commit"
                    },
                    "subdir": { "type": "string" },
                    "force": {
                        "type": "boolean",
                        "default": false
                    }
                },
                "required": ["url"],
                "additionalProperties": false
            },
            "annotations": { "destructiveHint": true, "openWorldHint": true }
        }),
        json!({
            "name": "list_sources",
            "title": "List sources",
            "description": "List the configured source roots, their kinds, and whether they exist.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "server_health",
            "title": "Server health",
            "description": "Report server health: version, protocol, config path, source existence, per-source and per-kind asset counts, and scan warnings.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true }
        }),
    ]
}

fn optional_kind(args: &Value) -> std::result::Result<Option<Kind>, String> {
    match args.get("kind") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Kind::parse_loose(value).map(Some).ok_or_else(|| {
            format!("unknown kind `{value}` (skill, agent, command, rule, snippet)")
        }),
        Some(_) => Err("`kind` must be a string".into()),
    }
}

fn required_kind(args: &Value) -> std::result::Result<Kind, String> {
    optional_kind(args)?.ok_or_else(|| "`kind` is required".into())
}

fn optional_str(args: &Value, key: &str) -> std::result::Result<Option<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.trim().is_empty() => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!("`{key}` must be a string")),
    }
}

fn required_str(args: &Value, key: &str) -> std::result::Result<String, String> {
    optional_str(args, key)?.ok_or_else(|| format!("`{key}` is required"))
}

fn required_text(args: &Value, key: &str) -> std::result::Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("`{key}` is required and must be a string"))
}

fn optional_bool(args: &Value, key: &str) -> std::result::Result<Option<bool>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(format!("`{key}` must be a boolean")),
    }
}

fn optional_usize(args: &Value, key: &str) -> std::result::Result<Option<usize>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(|value| Some(value as usize))
            .ok_or_else(|| format!("`{key}` must be a non-negative integer")),
    }
}

fn resolve_asset<'a>(assets: &'a [Asset], args: &Value) -> std::result::Result<&'a Asset, String> {
    let kind = optional_kind(args)?;
    let name = optional_str(args, "name")?;
    let path = optional_str(args, "path")?;
    if name.is_none() && path.is_none() {
        return Err("provide `path` or `name`".into());
    }
    let target = path
        .as_deref()
        .map(|path| canonical(&config::expand_path(path)));

    let matches: Vec<&Asset> = assets
        .iter()
        .filter(|asset| {
            if let Some(kind) = kind
                && asset.kind != kind
            {
                return false;
            }
            if let Some(target) = &target {
                let file = canonical(&asset.path) == *target;
                let directory = asset.kind == Kind::Skill
                    && asset
                        .path
                        .parent()
                        .is_some_and(|parent| canonical(parent) == *target);
                if !file && !directory {
                    return false;
                }
            }
            if let Some(name) = &name
                && !asset.name.eq_ignore_ascii_case(name)
            {
                return false;
            }
            true
        })
        .collect();

    match matches.as_slice() {
        [] => Err("no asset matches; call list_assets".into()),
        [asset] => Ok(asset),
        many => {
            let listing = many
                .iter()
                .map(|asset| {
                    format!(
                        "  {} {} ({})",
                        asset.kind.as_str(),
                        asset.name,
                        asset.path.display()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            Err(format!(
                "multiple assets match; narrow with kind, name, or path:\n{listing}"
            ))
        }
    }
}

fn removal_target(asset: &Asset) -> PathBuf {
    if asset.kind == Kind::Skill
        && asset
            .path
            .file_name()
            .is_some_and(|name| name == "SKILL.md")
        && let Some(parent) = asset.path.parent()
        && canonical(parent) != canonical(&config::expand_path(&asset.source))
    {
        return parent.to_path_buf();
    }
    asset.path.clone()
}

fn skill_files(directory: &Path) -> Vec<Value> {
    WalkDir::new(directory)
        .max_depth(12)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            let hidden = entry.depth() > 0 && name.starts_with('.');
            !hidden && name != ".git"
        })
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .take(MAX_SKILL_FILES)
        .map(|entry| {
            let relative = entry.path().strip_prefix(directory).unwrap_or(entry.path());
            json!({
                "path": relative.to_string_lossy(),
                "size": entry.metadata().map(|meta| meta.len()).ok()
            })
        })
        .collect()
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn file_uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let text = if text.starts_with('/') {
        text
    } else {
        format!("/{text}")
    };
    let mut uri = String::from("file://");
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.' | b'~' | b':') {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

fn path_from_file_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let mut bytes = Vec::with_capacity(rest.len());
    let mut iter = rest.bytes();
    while let Some(byte) = iter.next() {
        if byte == b'%' {
            let pair = [iter.next()?, iter.next()?];
            let pair = std::str::from_utf8(&pair).ok()?;
            bytes.push(u8::from_str_radix(pair, 16).ok()?);
        } else {
            bytes.push(byte);
        }
    }
    let text = String::from_utf8(bytes).ok()?;
    #[cfg(windows)]
    let text = text.trim_start_matches('/').to_string();
    Some(PathBuf::from(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(root: &Path) -> Server {
        let mut cfg = Config {
            sources: vec![Source::new(Kind::Skill, root.to_string_lossy())],
            ..Config::default()
        };
        cfg.behavior.trash = false;
        Server::new(cfg, root.join("config.toml"))
    }

    fn call(server: &mut Server, name: &str, arguments: Value) -> Value {
        let response = server
            .handle(&json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "tools/call",
                "params": { "name": name, "arguments": arguments }
            }))
            .expect("response");
        assert!(response.get("error").is_none(), "{response}");
        let result = &response["result"];
        assert_eq!(result["isError"], false, "{result}");
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
    }

    #[test]
    fn initializes_with_latest_protocol() {
        let directory = tempfile::tempdir().unwrap();
        let mut server = server(directory.path());
        let response = server
            .handle(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": "2025-11-25" }
            }))
            .unwrap();
        assert_eq!(response["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(response["result"]["serverInfo"]["name"], "arsenal");
        assert!(response["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn creates_reads_writes_lists_and_deletes_a_skill() {
        let directory = tempfile::tempdir().unwrap();
        let mut server = server(directory.path());

        let created = call(
            &mut server,
            "create_asset",
            json!({ "kind": "skill", "name": "Deploy Service", "description": "Deploy safely" }),
        );
        assert_eq!(created["created"], true);
        let path = created["path"].as_str().unwrap().to_string();
        assert!(Path::new(&path).is_file());

        let read = call(&mut server, "read_asset", json!({ "path": path }));
        assert_eq!(read["asset"]["kind"], "skill");
        assert!(read["content"].as_str().unwrap().contains("Deploy safely"));
        assert!(!read["files"].as_array().unwrap().is_empty());

        let listing = call(&mut server, "list_assets", json!({ "query": "deploy" }));
        assert_eq!(listing["total"], 1);

        let updated = "---\nname: deploy-service\ndescription: Updated\n---\n\n# Updated\n";
        let written = call(
            &mut server,
            "write_asset",
            json!({ "path": path, "content": updated }),
        );
        assert_eq!(written["updated"], true);
        let reread = call(
            &mut server,
            "read_asset",
            json!({ "name": "deploy-service" }),
        );
        assert_eq!(reread["content"], updated);

        let deleted = call(
            &mut server,
            "delete_asset",
            json!({ "path": path, "purge": true }),
        );
        assert_eq!(deleted["deleted"], true);
        assert!(!Path::new(directory.path()).join("deploy-service").exists());
    }

    #[test]
    fn rejects_paths_outside_configured_sources() {
        let directory = tempfile::tempdir().unwrap();
        let outside = directory.path().join("outside.md");
        fs::write(&outside, "---\nname: outside\n---\n").unwrap();
        let mut server = server(&directory.path().join("skills"));

        let response = server
            .handle(&json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "write_asset",
                    "arguments": { "path": outside, "content": "changed" }
                }
            }))
            .unwrap();
        assert_eq!(response["result"]["isError"], true);
        assert_eq!(
            fs::read_to_string(&outside).unwrap(),
            "---\nname: outside\n---\n"
        );
    }

    #[test]
    fn exposes_assets_as_resources() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("review.md"),
            "---\nname: review\ndescription: Review code\n---\n\nBody\n",
        )
        .unwrap();
        let mut server = server(directory.path());

        let listed = server
            .handle(&json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "resources/list"
            }))
            .unwrap();
        let uri = listed["result"]["resources"][0]["uri"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(uri.starts_with("file://"));

        let read = server
            .handle(&json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "resources/read",
                "params": { "uri": uri }
            }))
            .unwrap();
        assert!(
            read["result"]["contents"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Review code")
        );
    }

    #[test]
    fn ping_and_health_tool_are_available() {
        let directory = tempfile::tempdir().unwrap();
        let mut server = server(directory.path());

        let pong = server
            .handle(&json!({ "jsonrpc": "2.0", "id": 8, "method": "ping" }))
            .unwrap();
        assert_eq!(pong["result"], json!({}));

        let tools = server
            .handle(&json!({ "jsonrpc": "2.0", "id": 9, "method": "tools/list" }))
            .unwrap();
        let names: Vec<&str> = tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert!(names.contains(&"server_health"));

        let health = call(&mut server, "server_health", json!({}));
        assert_eq!(health["status"], "ok");
        assert_eq!(health["healthy"], true);
        assert_eq!(health["sources_active"], 1);
        assert_eq!(health["assets"]["skill"], 0);
    }

    #[test]
    fn health_is_unhealthy_without_source_directories() {
        let cfg = Config {
            sources: Vec::new(),
            ..Config::default()
        };
        let (value, healthy) = health_value(&cfg, Path::new("/tmp/config.toml"));
        assert!(!healthy);
        assert_eq!(value["status"], "unhealthy");
        assert_eq!(value["sources_total"], 0);
    }

    #[test]
    fn unknown_methods_and_notifications() {
        let directory = tempfile::tempdir().unwrap();
        let mut server = server(directory.path());
        let response = server
            .handle(&json!({ "jsonrpc": "2.0", "id": 5, "method": "nope" }))
            .unwrap();
        assert_eq!(response["error"]["code"], -32601);
        assert!(
            server
                .handle(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
                .is_none()
        );
    }
}
