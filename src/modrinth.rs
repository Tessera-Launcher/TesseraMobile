//! A narrow slice of Modrinth v2: TouchController plus its required deps.
//!
//! This is intentionally not the desktop launcher's full Modrinth page. The
//! phone install only needs one graph: the newest TouchController build for
//! the selected Minecraft version, then every required Fabric client
//! dependency at the newest compatible version. The resulting jars are added
//! to the MDT download list, so aria2c still moves all the bytes.

use std::collections::{HashSet, VecDeque};

use serde::Deserialize;

const API: &str = "https://api.modrinth.com/v2";
/// Modrinth resolves this slug to project `U7KwGAnT`.
const TOUCH_CONTROLLER: &str = "touchcontroller";
/// Fabric API is normally added from Fabric Maven before this resolver runs.
pub const FABRIC_API_PROJECT: &str = "P7dR8mSH";
const MAX_DEPENDENCY_DEPTH: usize = 8;

/// One jar to append to the game install plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModFile {
    pub filename: String,
    pub url: String,
    pub size: u64,
    pub sha1: Option<String>,
    pub project_id: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Version {
    id: String,
    #[serde(default)]
    project_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    date_published: String,
    #[serde(default)]
    game_versions: Vec<String>,
    #[serde(default)]
    loaders: Vec<String>,
    #[serde(default)]
    environment: Option<String>,
    #[serde(default)]
    files: Vec<File>,
    #[serde(default)]
    dependencies: Vec<Dependency>,
}

#[derive(Debug, Clone, Deserialize)]
struct File {
    #[serde(default)]
    hashes: Hashes,
    #[serde(default)]
    url: String,
    #[serde(default)]
    filename: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    primary: bool,
    #[serde(default)]
    file_type: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct Hashes {
    #[serde(default)]
    sha1: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Dependency {
    #[serde(default)]
    version_id: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    file_name: Option<String>,
    #[serde(default)]
    dependency_type: String,
}

impl Version {
    fn primary_file(&self) -> Option<&File> {
        self.files
            .iter()
            .find(|file| file.primary && file.is_mod_jar())
            .or_else(|| self.files.iter().find(|file| file.is_mod_jar()))
    }

    fn is_client_compatible(&self) -> bool {
        self.environment.as_deref().is_none_or(|environment| {
            !matches!(
                environment,
                "server_only" | "server_only_client_optional" | "dedicated_server_only"
            )
        })
    }

    fn is_compatible_with(&self, mc_version: &str) -> bool {
        self.primary_file().is_some()
            && self.loaders.iter().any(|loader| loader == "fabric")
            && self.game_versions.iter().any(|game| game == mc_version)
            && self.is_client_compatible()
    }
}

impl File {
    fn is_mod_jar(&self) -> bool {
        let name = self.filename.to_ascii_lowercase();
        let excluded_type = matches!(
            self.file_type.as_deref(),
            Some("sources-jar" | "dev-jar" | "javadoc-jar" | "signature")
        );
        name.ends_with(".jar")
            && !excluded_type
            && !["-sources", "-javadoc", "-dev"]
                .iter()
                .any(|suffix| name.contains(suffix))
    }
}

/// Resolve TouchController and all required dependencies for one game version.
///
/// A missing TouchController or required dependency is an error: installing a
/// Fabric instance without touch input would produce a game that cannot be
/// controlled at all.
pub async fn touch_controller(mc_version: &str) -> Result<Vec<ModFile>, String> {
    let client = crate::http::client()?;
    let versions = project_versions(&client, TOUCH_CONTROLLER, mc_version).await?;
    let root = versions
        .into_iter()
        .find(Version::is_client_compatible)
        .ok_or_else(|| format!("TouchController 没有适用于 {mc_version} 的 Fabric 客户端版本"))?;

    let mut queue = VecDeque::from([(root, 0usize)]);
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    while let Some((version, depth)) = queue.pop_front() {
        if depth > MAX_DEPENDENCY_DEPTH {
            return Err(format!(
                "TouchController 依赖层级超过 {MAX_DEPENDENCY_DEPTH}：{}",
                version.name
            ));
        }
        let key = if version.project_id.is_empty() {
            version.id.clone()
        } else {
            version.project_id.clone()
        };
        if !seen.insert(key) {
            continue;
        }
        let file = version
            .primary_file()
            .ok_or_else(|| format!("{} 没有可下载的 jar", version.name))?;
        let filename = safe_file_name(&file.filename)
            .ok_or_else(|| format!("Modrinth 返回了非法文件名：{}", file.filename))?;
        out.push(ModFile {
            filename,
            url: file.url.clone(),
            size: file.size,
            sha1: file
                .hashes
                .sha1
                .as_deref()
                .map(str::to_ascii_lowercase)
                .filter(|sha1| sha1.len() == 40),
            project_id: version.project_id.clone(),
        });

        for dependency in version
            .dependencies
            .iter()
            .filter(|dependency| dependency.dependency_type == "required")
        {
            let Some(resolved) = resolve_dependency(&client, dependency, mc_version).await else {
                return Err(format!(
                    "{} 的必需依赖无法解析：{}",
                    version.name,
                    dependency
                        .project_id
                        .as_deref()
                        .or(dependency.file_name.as_deref())
                        .unwrap_or("（未命名）")
                ));
            };
            queue.push_back((resolved, depth + 1));
        }
    }
    Ok(out)
}

async fn resolve_dependency(
    client: &reqwest::Client,
    dependency: &Dependency,
    mc_version: &str,
) -> Option<Version> {
    if let Some(version_id) = &dependency.version_id {
        if let Ok(version) = version_by_id(client, version_id).await {
            if version.is_compatible_with(mc_version) {
                return Some(version);
            }
        }
    }
    let project = dependency.project_id.as_deref()?;
    let versions = project_versions(client, project, mc_version).await.ok()?;
    versions.into_iter().next()
}

async fn project_versions(
    client: &reqwest::Client,
    project: &str,
    mc_version: &str,
) -> Result<Vec<Version>, String> {
    let mut url = reqwest::Url::parse(&format!("{API}/project/{project}/version"))
        .map_err(|error| format!("无法构造 Modrinth 请求：{error}"))?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("loaders", "[\"fabric\"]");
        query.append_pair("game_versions", &format!("[\"{mc_version}\"]"));
        query.append_pair("include_changelog", "false");
    }
    let versions = get_json::<Vec<Version>>(client, url, "Modrinth 版本列表").await?;
    let mut versions: Vec<Version> = versions
        .into_iter()
        .filter(|version| version.is_compatible_with(mc_version))
        .collect();
    versions.sort_by(|a, b| b.date_published.cmp(&a.date_published));
    Ok(versions)
}

async fn version_by_id(client: &reqwest::Client, version_id: &str) -> Result<Version, String> {
    let url = reqwest::Url::parse(&format!("{API}/version/{version_id}"))
        .map_err(|error| format!("无法构造 Modrinth 请求：{error}"))?;
    get_json(client, url, "Modrinth 依赖版本").await
}

async fn get_json<T: for<'de> Deserialize<'de>>(
    client: &reqwest::Client,
    url: reqwest::Url,
    what: &str,
) -> Result<T, String> {
    let response = client
        .get(url.clone())
        .send()
        .await
        .map_err(|error| format!("请求 {what} 失败：{error}"))?;
    if !response.status().is_success() {
        return Err(format!("{what}返回 HTTP {}", response.status()));
    }
    response
        .json::<T>()
        .await
        .map_err(|error| format!("{what}无法解析：{error}"))
}

fn safe_file_name(filename: &str) -> Option<String> {
    if filename.is_empty()
        || filename == "."
        || filename == ".."
        || filename.contains(['/', '\\', '\0'])
    {
        return None;
    }
    Some(filename.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_environment_filter_rejects_server_only() {
        let mut version = Version {
            id: "v".into(),
            project_id: "p".into(),
            name: "name".into(),
            date_published: String::new(),
            game_versions: vec!["26.3".into()],
            loaders: vec!["fabric".into()],
            environment: Some("server_only".into()),
            files: vec![File {
                hashes: Hashes::default(),
                url: "https://cdn.example/x.jar".into(),
                filename: "x.jar".into(),
                size: 1,
                primary: true,
                file_type: None,
            }],
            dependencies: Vec::new(),
        };
        assert!(!version.is_compatible_with("26.3"));
        version.environment = Some("client_and_server".into());
        assert!(version.is_compatible_with("26.3"));
    }

    #[test]
    fn filenames_cannot_escape_the_mods_directory() {
        assert_eq!(
            safe_file_name("TouchController.jar").as_deref(),
            Some("TouchController.jar")
        );
        assert_eq!(safe_file_name("../x.jar"), None);
        assert_eq!(safe_file_name("a/b.jar"), None);
    }
}
