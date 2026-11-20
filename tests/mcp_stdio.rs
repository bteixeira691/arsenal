//! End-to-end tests for the `arsenal mcp` stdio transport.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

struct Mcp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Mcp {
    fn start(config: &Path, skills: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_arsenal"))
            .arg("--config")
            .arg(config)
            .arg("--root")
            .arg(format!("skill={}", skills.display()))
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn arsenal mcp");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Mcp {
            child,
            stdin,
            stdout,
        }
    }

    fn send_line(&mut self, line: &str) {
        writeln!(self.stdin, "{line}").expect("write request");
        self.stdin.flush().expect("flush request");
    }

    fn send(&mut self, message: &Value) {
        self.send_line(&message.to_string());
    }

    fn recv(&mut self) -> Value {
        let mut line = String::new();
        assert!(
            self.stdout.read_line(&mut line).expect("read response") > 0,
            "server closed stdout"
        );
        serde_json::from_str(&line).unwrap_or_else(|error| panic!("bad response {line:?}: {error}"))
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }));
        let response = self.recv();
        assert_eq!(response["id"], id, "{response}");
        response
    }

    fn tool(&mut self, id: u64, name: &str, arguments: Value) -> Value {
        let response = self.request(
            id,
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        );
        assert_eq!(response["result"]["isError"], false, "{response}");
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
            .expect("tool payload")
    }

    fn close(self) -> std::process::Output {
        drop(self.stdin);
        self.child.wait_with_output().expect("wait for server")
    }
}

#[test]
fn mcp_stdio_round_trip() {
    let directory = tempfile::tempdir().unwrap();
    let skills = directory.path().join("skills");
    fs::create_dir_all(&skills).unwrap();
    let mut mcp = Mcp::start(&directory.path().join("config.toml"), &skills);

    let initialize = mcp.request(
        1,
        "initialize",
        json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "arsenal-test", "version": "1.0.0" }
        }),
    );
    assert_eq!(initialize["result"]["protocolVersion"], json!("2025-11-25"));
    assert_eq!(initialize["result"]["serverInfo"]["name"], json!("arsenal"));

    mcp.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    assert_eq!(mcp.request(2, "ping", json!({}))["result"], json!({}));

    let tools = mcp.request(3, "tools/list", json!({}));
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 8);

    let created = mcp.tool(
        4,
        "create_asset",
        json!({ "kind": "skill", "name": "Deploy", "description": "Deploy services" }),
    );
    let path = created["path"].as_str().unwrap().to_string();
    assert!(Path::new(&path).is_file());

    let listing = mcp.tool(5, "list_assets", json!({ "query": "deploy" }));
    assert_eq!(listing["total"], json!(1));

    let read = mcp.tool(6, "read_asset", json!({ "name": "deploy" }));
    assert!(
        read["content"]
            .as_str()
            .unwrap()
            .contains("Deploy services")
    );

    let updated = "---\nname: deploy\ndescription: Updated\n---\n\n# Updated\n";
    let written = mcp.tool(
        7,
        "write_asset",
        json!({ "path": path, "content": updated }),
    );
    assert_eq!(written["updated"], json!(true));
    let reread = mcp.tool(8, "read_asset", json!({ "name": "deploy" }));
    assert_eq!(reread["content"], json!(updated));

    let health = mcp.tool(9, "server_health", json!({}));
    assert_eq!(health["status"], json!("ok"));

    let resources = mcp.request(10, "resources/list", json!({}));
    let resources = resources["result"]["resources"].as_array().unwrap();
    let uri = resources
        .iter()
        .find(|resource| resource["name"].as_str().unwrap().ends_with("deploy"))
        .unwrap()["uri"]
        .as_str()
        .unwrap()
        .to_string();
    let resource = mcp.request(11, "resources/read", json!({ "uri": uri }));
    assert!(
        resource["result"]["contents"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Updated")
    );

    mcp.send_line("{not json");
    let parse_error = mcp.recv();
    assert_eq!(parse_error["error"]["code"], json!(-32700));
    assert_eq!(parse_error["id"], Value::Null);
    assert_eq!(mcp.request(12, "ping", json!({}))["result"], json!({}));

    let unknown = mcp.request(13, "does/not/exist", json!({}));
    assert_eq!(unknown["error"]["code"], json!(-32601));

    let deleted = mcp.tool(14, "delete_asset", json!({ "path": path, "purge": true }));
    assert_eq!(deleted["deleted"], json!(true));
    assert!(!Path::new(&path).parent().unwrap().exists());

    let output = mcp.close();
    assert!(output.status.success(), "{output:?}");
}
