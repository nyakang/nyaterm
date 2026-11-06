//! Stateful wire fixture, with pagination, staged objects and upload sessions.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use nyaterm_core::cloud_sync::credentials::DriveProvider;
use serde_json::{Value, json};

use crate::test_support::http::{Reply, Request, Server};

#[derive(Clone)]
pub struct Node {
    pub parent: String,
    pub name: String,
    pub folder: bool,
    pub bytes: Vec<u8>,
    pub complete: bool,
}
pub struct State {
    pub nodes: BTreeMap<String, Node>,
    next_id: usize,
    pub required_token: String,
    pub api_status: Option<u16>,
    pub redirect_downloads: bool,
    pub bad_next_link: bool,
    pub refresh_status: u16,
    pub rotated_refresh: bool,
    pub mutation_failure: Option<u16>,
    pub lose_move_reply: bool,
    pub incomplete: bool,
    pub cycle: bool,
    pub final_chunk_status: u16,
}
impl Default for State {
    fn default() -> Self {
        let root = Node {
            parent: String::new(),
            name: "root".into(),
            folder: true,
            bytes: Vec::new(),
            complete: true,
        };
        Self {
            nodes: [("root".into(), root)].into(),
            next_id: 1,
            required_token: "access".into(),
            api_status: None,
            redirect_downloads: false,
            bad_next_link: false,
            refresh_status: 200,
            rotated_refresh: true,
            mutation_failure: None,
            lose_move_reply: false,
            incomplete: false,
            cycle: false,
            final_chunk_status: 200,
        }
    }
}
pub struct DriveFixture {
    pub server: Server,
    pub state: Arc<Mutex<State>>,
}
impl Default for DriveFixture {
    fn default() -> Self {
        Self::new()
    }
}
impl DriveFixture {
    pub fn new() -> Self {
        let state = Arc::new(Mutex::new(State::default()));
        let cloned = state.clone();
        let server = Server::new(move |req| cloned.lock().unwrap().respond(req));
        Self { server, state }
    }
}
fn reply(status: u16, value: Value) -> Reply {
    Reply::new(status, serde_json::to_vec(&value).unwrap())
        .header("content-type", "application/json")
}
fn missing() -> Reply {
    reply(
        404,
        json!({"error": {"code": "itemNotFound"}, "code": "NotFound.File"}),
    )
}
impl State {
    pub fn insert(&mut self, parent: &str, name: &str, folder: bool, bytes: Vec<u8>) -> String {
        let id = format!("id{}", self.next_id);
        self.next_id += 1;
        self.nodes.insert(
            id.clone(),
            Node {
                parent: parent.into(),
                name: name.into(),
                folder,
                bytes,
                complete: true,
            },
        );
        id
    }
    fn find(&self, full: &str) -> Option<String> {
        let mut parent = "root".to_owned();
        for segment in full.trim_matches('/').split('/').filter(|s| !s.is_empty()) {
            parent = self
                .nodes
                .iter()
                .find(|(_, n)| n.parent == parent && n.name == segment && n.complete)?
                .0
                .clone();
        }
        Some(parent)
    }
    fn item(&self, id: &str, provider: DriveProvider) -> Value {
        let node = &self.nodes[id];
        match provider {
            DriveProvider::Google => {
                json!({"id":id,"name":node.name,"mimeType": if node.folder {"application/vnd.google-apps.folder"} else {"application/octet-stream"},"size":node.bytes.len().to_string(),"modifiedTime":"2026-01-01T00:00:00Z","parents":[node.parent]})
            }
            DriveProvider::OneDrive => {
                let mut value = json!({"id":id,"name":node.name,"size":node.bytes.len(),"eTag":format!("etag-{}",node.bytes.len()),"lastModifiedDateTime":"2026-01-01T00:00:00Z","parentReference":{"id":node.parent,"driveId":"drive","path":"/drive/root:"}});
                if node.folder {
                    value["folder"] = json!({"childCount":0});
                } else {
                    value["file"] = json!({"mimeType":"application/octet-stream"});
                }
                value
            }
            DriveProvider::Aliyun => {
                json!({"file_id":id,"parent_file_id":node.parent,"name":node.name,"type":if node.folder {"folder"} else {"file"},"size":node.bytes.len(),"updated_at":"2026-01-01T00:00:00Z"})
            }
        }
    }
    fn move_item(
        &mut self,
        id: &str,
        parent: &str,
        name: Option<&str>,
        provider: DriveProvider,
    ) -> Reply {
        let Some(node) = self.nodes.get(id) else {
            return missing();
        };
        let name = name.unwrap_or(&node.name).to_owned();
        if provider != DriveProvider::Google
            && self
                .nodes
                .iter()
                .any(|(other, n)| other != id && n.parent == parent && n.name == name)
        {
            return reply(409, json!({}));
        }
        let node = self.nodes.get_mut(id).unwrap();
        node.parent = parent.into();
        node.name = name;
        if self.lose_move_reply {
            self.lose_move_reply = false;
            let mut res = Reply::new(200, b"{".to_vec());
            res.advertised_length = Some(50);
            return res;
        }
        reply(200, self.item(id, provider))
    }
    fn respond(&mut self, req: &Request) -> Reply {
        let url = url::Url::parse(&format!("http://fixture{}", req.target)).unwrap();
        let path = url.path();
        let query: BTreeMap<String, String> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let data: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        if path.starts_with("/oauth2.googleapis.com/")
            || path.starts_with("/login.microsoftonline.com/")
            || path.ends_with("/oauth/access_token")
        {
            if self.refresh_status != 200 {
                return reply(
                    self.refresh_status,
                    json!({"error":"invalid_grant","error_description":"must never escape"}),
                );
            }
            self.required_token = "refreshed".into();
            let mut token = json!({"access_token":"refreshed","expires_in":3600});
            if self.rotated_refresh {
                token["refresh_token"] = json!("rotated");
            }
            return reply(200, token);
        }
        if path.starts_with("/uploads.example/") || path.starts_with("/downloads.example/") {
            assert!(
                !req.headers.contains_key("authorization"),
                "signed URL received credentials"
            );
            let id = path.rsplit('/').next().unwrap();
            if req.method == "GET" {
                let Some(node) = self.nodes.get(id) else {
                    return missing();
                };
                return Reply::new(200, node.bytes.clone());
            }
            if let Some(status) = self.mutation_failure.take() {
                return reply(status, json!({}));
            }
            let Some(node) = self.nodes.get_mut(id) else {
                return missing();
            };
            if let Some(range) = req.headers.get("content-range") {
                let (span, total) = range
                    .strip_prefix("bytes ")
                    .unwrap()
                    .split_once('/')
                    .unwrap();
                let (start, end) = span.split_once('-').unwrap();
                assert_eq!(start.parse::<usize>().unwrap(), node.bytes.len());
                node.bytes.extend_from_slice(&req.body);
                if end.parse::<usize>().unwrap() + 1 == total.parse::<usize>().unwrap() {
                    node.complete = self.final_chunk_status != 202;
                    return reply(
                        self.final_chunk_status,
                        self.item(id, DriveProvider::OneDrive),
                    );
                }
                return reply(
                    202,
                    json!({"nextExpectedRanges":[format!("{}-",node.bytes.len())]}),
                );
            }
            node.bytes.extend_from_slice(&req.body);
            return Reply::new(200, Vec::new());
        }
        if req.headers.get("authorization").map(String::as_str)
            != Some(&format!("Bearer {}", self.required_token))
        {
            return reply(401, json!({}));
        }
        if let Some(status) = self.api_status {
            return reply(status, json!({"untrusted":"secret body"})).header("retry-after", "17");
        }
        let provider = if path.starts_with("/www.googleapis.com/") {
            DriveProvider::Google
        } else if path.starts_with("/graph.microsoft.com/") {
            DriveProvider::OneDrive
        } else {
            DriveProvider::Aliyun
        };
        if provider == DriveProvider::Google {
            if path == "/www.googleapis.com/drive/v3/files" && req.method == "GET" {
                let q = query.get("q").map(String::as_str).unwrap_or_default();
                let parent = q
                    .split("' in parents")
                    .next()
                    .unwrap()
                    .rsplit('\'')
                    .next()
                    .unwrap();
                let wanted = q
                    .split("name = '")
                    .nth(1)
                    .and_then(|v| v.split('\'').next());
                let items: Vec<_> = self
                    .nodes
                    .iter()
                    .filter(|(_, n)| {
                        n.parent == parent && n.complete && wanted.is_none_or(|name| name == n.name)
                    })
                    .map(|(id, _)| self.item(id, provider))
                    .collect();
                let start = query
                    .get("pageToken")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                let end = (start + 2).min(items.len());
                let mut result =
                    json!({"files": &items[start..end], "incompleteSearch":self.incomplete});
                if end < items.len() || self.cycle {
                    result["nextPageToken"] = json!(if self.cycle {
                        "0".into()
                    } else {
                        end.to_string()
                    });
                }
                return reply(200, result);
            }
            if path.starts_with("/www.googleapis.com/upload/drive/v3/files") {
                if let Some(status) = self.mutation_failure.take() {
                    return reply(status, json!({}));
                }
                if req.method == "PATCH" {
                    let id = path.rsplit('/').next().unwrap();
                    let Some(node) = self.nodes.get_mut(id) else {
                        return missing();
                    };
                    node.bytes = req.body.clone();
                    return reply(200, self.item(id, provider));
                }
                let boundary = req.headers["content-type"]
                    .split("boundary=")
                    .nth(1)
                    .unwrap()
                    .trim_matches('"');
                let marker = format!("--{boundary}").into_bytes();
                let starts: Vec<usize> = req
                    .body
                    .windows(marker.len())
                    .enumerate()
                    .filter_map(|(i, w)| (w == marker).then_some(i))
                    .collect();
                let extract = |index: usize| {
                    let part = &req.body[starts[index] + marker.len()..starts[index + 1] - 2];
                    let head = part.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
                    &part[head + 4..]
                };
                let meta: Value = serde_json::from_slice(extract(0)).unwrap();
                let id = self.insert(
                    meta["parents"][0].as_str().unwrap(),
                    meta["name"].as_str().unwrap(),
                    false,
                    extract(1).to_vec(),
                );
                return reply(200, self.item(&id, provider));
            }
            if path == "/www.googleapis.com/drive/v3/files" && req.method == "POST" {
                let id = self.insert(
                    data["parents"][0].as_str().unwrap(),
                    data["name"].as_str().unwrap(),
                    true,
                    Vec::new(),
                );
                return reply(200, self.item(&id, provider));
            }
            let id = path.rsplit('/').next().unwrap();
            if req.method == "PATCH" {
                if data["trashed"] == true {
                    self.nodes.remove(id);
                    return reply(200, json!({}));
                }
                return self.move_item(id, query.get("addParents").unwrap(), None, provider);
            }
            if query.get("alt").is_some_and(|s| s == "media") {
                return self
                    .nodes
                    .get(id)
                    .map(|n| Reply::new(200, n.bytes.clone()))
                    .unwrap_or_else(missing);
            }
            return self
                .nodes
                .get(id)
                .map(|_| reply(200, self.item(id, provider)))
                .unwrap_or_else(missing);
        }
        if provider == DriveProvider::OneDrive {
            let tail = path
                .strip_prefix("/graph.microsoft.com/v1.0/me/drive/")
                .unwrap();
            let (address, action) = if let Some((a, b)) = tail.split_once(":/") {
                let (name, action) = b.split_once(":/").unwrap_or((b, ""));
                (format!("{a}:/{name}"), action)
            } else if tail.starts_with("items/") {
                let mut parts = tail.splitn(3, '/');
                parts.next();
                (
                    format!("items/{}", parts.next().unwrap()),
                    parts.next().unwrap_or(""),
                )
            } else if tail == "root/children" {
                ("root".into(), "children")
            } else {
                (tail.into(), "")
            };
            let full = address.strip_prefix("root:/").map(|v| {
                url::form_urlencoded::parse(format!("p={}", v.replace('+', "%2B")).as_bytes())
                    .next()
                    .unwrap()
                    .1
                    .into_owned()
            });
            let id = if let Some(full) = &full {
                self.find(full)
            } else if address == "root" {
                Some("root".into())
            } else {
                address
                    .strip_prefix("items/")
                    .filter(|id| self.nodes.contains_key(*id))
                    .map(str::to_owned)
            };
            if action == "children" {
                let Some(id) = id else {
                    return missing();
                };
                if req.method == "POST" {
                    let name = data["name"].as_str().unwrap();
                    if self
                        .nodes
                        .values()
                        .any(|n| n.parent == id && n.name == name)
                    {
                        return reply(409, json!({}));
                    }
                    let created = self.insert(&id, name, true, Vec::new());
                    return reply(201, self.item(&created, provider));
                }
                let items: Vec<_> = self
                    .nodes
                    .iter()
                    .filter(|(_, n)| n.parent == id && n.complete)
                    .map(|(id, _)| self.item(id, provider))
                    .collect();
                let start = query.get("skip").and_then(|v| v.parse().ok()).unwrap_or(0);
                let end = (start + 2).min(items.len());
                let mut result = json!({"value": &items[start..end]});
                if end < items.len() || self.cycle {
                    result["@odata.nextLink"] = json!(format!(
                        "https://graph.microsoft.com{}?skip={}",
                        path.strip_prefix("/graph.microsoft.com").unwrap(),
                        if self.cycle { 0 } else { end }
                    ));
                }
                if self.bad_next_link {
                    result["@odata.nextLink"] = json!("https://attacker.example/steal");
                }
                return reply(200, result);
            }
            if action == "content" || action == "createUploadSession" {
                let Some(full) = full else {
                    return missing();
                };
                if req.method == "GET" {
                    if let Some(ref id) = id {
                        let tag = format!("etag-{}", self.nodes[id].bytes.len());
                        if req.headers.get("if-none-match") == Some(&tag) {
                            return Reply::new(304, Vec::new());
                        }
                        if self.redirect_downloads {
                            return Reply::new(302, Vec::new())
                                .header("location", &format!("https://downloads.example/{id}"));
                        }
                        return Reply::new(200, self.nodes[id].bytes.clone()).header("etag", &tag);
                    }
                    return id
                        .and_then(|id| self.nodes.get(&id))
                        .map(|n| Reply::new(200, n.bytes.clone()))
                        .unwrap_or_else(missing);
                }
                if let Some(status) = self.mutation_failure.take() {
                    return reply(status, json!({}));
                }
                let (parent, name) = full.rsplit_once('/').unwrap_or(("", &full));
                let Some(parent) = self.find(parent) else {
                    return missing();
                };
                let id = if let Some(id) = id {
                    id
                } else {
                    self.insert(&parent, name, false, Vec::new())
                };
                if action == "createUploadSession" {
                    let node = self.nodes.get_mut(&id).unwrap();
                    node.complete = false;
                    node.bytes.clear();
                    return reply(
                        200,
                        json!({"uploadUrl":format!("https://uploads.example/{id}"),"expirationDateTime":"2030-01-01T00:00:00Z"}),
                    );
                }
                self.nodes.get_mut(&id).unwrap().bytes = req.body.clone();
                return reply(201, self.item(&id, provider));
            }
            let Some(id) = id else {
                return missing();
            };
            if req.method == "PATCH" {
                assert_eq!(
                    query
                        .get("@microsoft.graph.conflictBehavior")
                        .map(String::as_str),
                    Some("fail")
                );
                return self.move_item(
                    &id,
                    data["parentReference"]["id"].as_str().unwrap(),
                    data["name"].as_str(),
                    provider,
                );
            }
            if req.method == "DELETE" {
                self.nodes.remove(&id);
                return Reply::new(204, Vec::new());
            }
            return reply(200, self.item(&id, provider));
        }
        let action = path.rsplit('/').next().unwrap();
        if action == "getDriveInfo" {
            return reply(
                200,
                json!({"default_drive_id":"drive","resource_drive_id":"resource","backup_drive_id":"backup"}),
            );
        }
        if action == "get_by_path" {
            return self
                .find(data["file_path"].as_str().unwrap())
                .map(|id| reply(200, self.item(&id, provider)))
                .unwrap_or_else(|| reply(400, json!({"code":"NotFound.File"})));
        }
        if action == "create" {
            let parent = data["parent_file_id"].as_str().unwrap();
            let name = data["name"].as_str().unwrap();
            if let Some((id, _)) = self
                .nodes
                .iter()
                .find(|(_, n)| n.parent == parent && n.name == name)
            {
                return reply(200, json!({"file_id":id,"exist":true}));
            }
            let folder = data["type"] == "folder";
            let id = self.insert(parent, name, folder, Vec::new());
            self.nodes.get_mut(&id).unwrap().complete = folder;
            return reply(
                200,
                json!({"file_id":id,"upload_id":if folder{Value::Null}else{json!("upload")},"exist":false}),
            );
        }
        if action == "list" {
            let parent = data["parent_file_id"].as_str().unwrap();
            let items: Vec<_> = self
                .nodes
                .iter()
                .filter(|(_, n)| n.parent == parent && n.complete)
                .map(|(id, _)| self.item(id, provider))
                .collect();
            let start = data["marker"]
                .as_str()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let end = (start + 2).min(items.len());
            return reply(
                200,
                json!({"items": &items[start..end],"next_marker":if self.cycle{"0".into()}else if end<items.len(){end.to_string()}else{String::new()}}),
            );
        }
        let id = data["file_id"].as_str().unwrap_or_default();
        if !self.nodes.contains_key(id) {
            return missing();
        }
        match action {
            "getUploadUrl" => reply(
                200,
                json!({"part_info_list":[{"part_number":1,"upload_url":format!("https://uploads.example/{id}")}]}),
            ),
            "getDownloadUrl" => reply(
                200,
                json!({"url":format!("https://downloads.example/{id}")}),
            ),
            "complete" => {
                self.nodes.get_mut(id).unwrap().complete = true;
                reply(200, self.item(id, provider))
            }
            "move" => {
                assert_eq!(data["check_name_mode"], "refuse");
                self.move_item(
                    id,
                    data["to_parent_file_id"].as_str().unwrap(),
                    None,
                    provider,
                )
            }
            "delete" => {
                self.nodes.remove(id);
                reply(200, json!({}))
            }
            _ => panic!("unexpected fixture operation {action}"),
        }
    }
}
