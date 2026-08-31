//! Remote command synthesis and parsing for the Docker manager.
//!
//! Everything is read by running docker itself on an exec channel
//! multiplexed on the pane's live SSH session: nothing is installed on
//! the host, no rc file is written, nothing is injected into the shell.
//!
//! The parser is pure (`&str` -> data types) so it unit-tests against
//! captured output without a network.

use super::model::{ComposeProject, DockerContainer, DockerData, DockerImage};

/// Field separator inside docker output lines.
///
/// Like tmux, docker container/image names structurally cannot contain
/// `:` as a standalone separator in the columns we read (names use
/// `/` not `:`), so it is safe as a delimiter.
const FIELD: &str = "|";

/// Marker printed when the host has no docker at all.
const NO_DOCKER: &str = "---ORYXIS-NO-DOCKER---";

/// Marker printed when docker is present but daemon not running.
const NO_DAEMON: &str = "---ORYXIS-NO-DAEMON---";

/// Separator between the containers, images, and compose sections.
const IMAGES_SEP: &str = "---ORYXIS-IMAGES---";
const COMPOSE_SEP: &str = "---ORYXIS-COMPOSE---";

/// The probe command: a single round trip that lists containers,
/// images, and detects compose files in the pane's CWD.
///
/// `cwd` is the remote shell's working directory (from OSC 7), used
/// to detect docker-compose files. `None` skips compose detection.
pub(crate) fn probe_command(cwd: Option<&str>) -> String {
    let containers_fmt = [
        "{{.Names}}", "{{.Image}}", "{{.Status}}", "{{.State}}", "{{.ID}}",
    ]
    .join(FIELD);
    let images_fmt = ["{{.Repository}}", "{{.Tag}}", "{{.Size}}", "{{.ID}}"].join(FIELD);

    // The compose detection looks for common compose filenames in cwd.
    let compose_check = match cwd {
        Some(dir) => {
            let dir = oryxis_archive::quote::sh_quote(dir).unwrap_or_else(|_| format!("'{dir}'"));
            format!(
                "if command -v docker >/dev/null 2>&1; then \
                 for f in docker-compose.yml docker-compose.yaml compose.yml compose.yaml; do \
                 if [ -f \"{dir}/$f\" ]; then echo \"{dir}/$f\"; fi; done; fi"
            )
        }
        None => String::new(),
    };

    let batch = format!(
        "command -v docker >/dev/null 2>&1 || {{ echo {NO_DOCKER}; exit 0; }}; \
         docker info >/dev/null 2>&1 || {{ echo {NO_DAEMON}; exit 0; }}; \
         docker ps -a --no-trunc --format \"{containers_fmt}\" 2>/dev/null; \
         echo {IMAGES_SEP}; \
         docker images --no-trunc --format \"{images_fmt}\" 2>/dev/null; \
         echo {COMPOSE_SEP}; \
         {compose_check}"
    );
    format!("sh -c '{batch}'")
}

/// What a probe answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProbeResult {
    /// Docker is not on the host's PATH.
    NoDocker,
    /// Docker is installed but the daemon is not running.
    NoDaemon,
    /// Docker answered with data.
    Data(DockerData),
}

/// Parse a probe payload. Unparseable lines are skipped rather than
/// failing the whole probe.
pub(crate) fn parse_probe(payload: &str) -> ProbeResult {
    if payload.lines().any(|l| l.trim() == NO_DOCKER) {
        return ProbeResult::NoDocker;
    }
    if payload.lines().any(|l| l.trim() == NO_DAEMON) {
        return ProbeResult::NoDaemon;
    }

    let mut containers = Vec::new();
    let mut images = Vec::new();
    let mut compose_projects = Vec::new();

    let mut section = 0usize; // 0=containers, 1=images, 2=compose
    for line in payload.lines() {
        let line = line.trim_end_matches(['\r', '\n']);
        let trimmed = line.trim();
        if trimmed == IMAGES_SEP {
            section = 1;
            continue;
        }
        if trimmed == COMPOSE_SEP {
            section = 2;
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }

        match section {
            0 => {
                if let Some(c) = parse_container_line(trimmed) {
                    containers.push(c);
                }
            }
            1 => {
                if let Some(img) = parse_image_line(trimmed) {
                    images.push(img);
                }
            }
            2 => {
                // Each line is a compose file path.
                if !trimmed.starts_with("---ORYXIS-") {
                    compose_projects.push(parse_compose_line(trimmed));
                }
            }
            _ => {}
        }
    }

    ProbeResult::Data(DockerData {
        containers,
        images,
        compose_projects,
    })
}

/// One container line: `name|image|status|state|id`.
fn parse_container_line(line: &str) -> Option<DockerContainer> {
    let parts: Vec<&str> = line.splitn(5, FIELD).collect();
    if parts.len() < 5 {
        return None;
    }
    let name = parts[0].to_string();
    if name.is_empty() {
        return None;
    }
    Some(DockerContainer {
        name,
        image: parts[1].to_string(),
        status: parts[2].to_string(),
        state: parts[3].to_string(),
        id: parts[4].to_string(),
    })
}

/// One image line: `repository|tag|size|id`.
fn parse_image_line(line: &str) -> Option<DockerImage> {
    let parts: Vec<&str> = line.splitn(4, FIELD).collect();
    if parts.len() < 4 {
        return None;
    }
    let repository = parts[0].to_string();
    if repository.is_empty() {
        return None;
    }
    Some(DockerImage {
        repository,
        tag: parts[1].to_string(),
        size: parts[2].to_string(),
        id: parts[3].to_string(),
    })
}

/// A compose file path line.
fn parse_compose_line(line: &str) -> ComposeProject {
    // Extract project name from directory path.
    let dir_name = std::path::Path::new(line)
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("default")
        .to_string();
    ComposeProject {
        file_path: line.to_string(),
        project_name: dir_name,
    }
}

/// Container management commands.

pub(crate) fn start_container_command(name: &str) -> Result<String, oryxis_archive::ArchiveError> {
    let name = oryxis_archive::quote::sh_quote(name)?;
    Ok(format!("docker start {name}"))
}

pub(crate) fn stop_container_command(name: &str) -> Result<String, oryxis_archive::ArchiveError> {
    let name = oryxis_archive::quote::sh_quote(name)?;
    Ok(format!("docker stop {name}"))
}

pub(crate) fn restart_container_command(
    name: &str,
) -> Result<String, oryxis_archive::ArchiveError> {
    let name = oryxis_archive::quote::sh_quote(name)?;
    Ok(format!("docker restart {name}"))
}

pub(crate) fn remove_container_command(name: &str) -> Result<String, oryxis_archive::ArchiveError> {
    let name = oryxis_archive::quote::sh_quote(name)?;
    Ok(format!("docker rm {name}"))
}

pub(crate) fn compose_up_command(path: &str) -> Result<String, oryxis_archive::ArchiveError> {
    let path = oryxis_archive::quote::sh_quote(path)?;
    // Determine the directory containing the compose file.
    let dir = std::path::Path::new(path.trim_matches('\'').trim_matches('"'))
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string());
    let dir = oryxis_archive::quote::sh_quote(&dir)?;
    Ok(format!("docker compose -f {path} -p {dir} up -d"))
}

pub(crate) fn compose_down_command(path: &str) -> Result<String, oryxis_archive::ArchiveError> {
    let path = oryxis_archive::quote::sh_quote(path)?;
    let dir = std::path::Path::new(path.trim_matches('\'').trim_matches('"'))
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string());
    let dir = oryxis_archive::quote::sh_quote(&dir)?;
    Ok(format!("docker compose -f {path} -p {dir} down"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_container_lines() {
        let line = "web|nginx:latest|Up 3 hours|running|abc123";
        let c = parse_container_line(line).unwrap();
        assert_eq!(c.name, "web");
        assert_eq!(c.image, "nginx:latest");
        assert_eq!(c.status, "Up 3 hours");
        assert_eq!(c.state, "running");
        assert_eq!(c.id, "abc123");
    }

    #[test]
    fn parses_image_lines() {
        let line = "nginx|latest|187MB|sha256:abc";
        let img = parse_image_line(line).unwrap();
        assert_eq!(img.repository, "nginx");
        assert_eq!(img.tag, "latest");
        assert_eq!(img.size, "187MB");
    }

    #[test]
    fn parses_compose_lines() {
        let p = parse_compose_line("/home/user/myproject/compose.yml");
        assert_eq!(p.file_path, "/home/user/myproject/compose.yml");
        assert_eq!(p.project_name, "myproject");
    }

    #[test]
    fn parse_probe_detects_no_docker() {
        assert_eq!(
            parse_probe(&format!("{NO_DOCKER}\n")),
            ProbeResult::NoDocker
        );
    }

    #[test]
    fn parse_probe_detects_no_daemon() {
        assert_eq!(
            parse_probe(&format!("{NO_DAEMON}\n")),
            ProbeResult::NoDaemon
        );
    }

    #[test]
    fn parse_probe_full_payload() {
        let payload = format!(
            "web{FIELD}nginx:latest{FIELD}Up 3 hours{FIELD}running{FIELD}abc123\n\
             {IMAGES_SEP}\n\
             nginx{FIELD}latest{FIELD}187MB{FIELD}sha256:abc\n\
             {COMPOSE_SEP}\n\
             /home/user/proj/compose.yml\n"
        );
        let ProbeResult::Data(data) = parse_probe(&payload) else {
            panic!("expected data");
        };
        assert_eq!(data.containers.len(), 1);
        assert_eq!(data.images.len(), 1);
        assert_eq!(data.compose_projects.len(), 1);
    }

    #[test]
    fn skip_unknown_field_lines() {
        // An unparseable line between sections is simply skipped.
        let payload = format!("garbage\n{IMAGES_SEP}\nnginx{FIELD}latest{FIELD}187MB{FIELD}sha256:abc\n{COMPOSE_SEP}\n");
        let ProbeResult::Data(data) = parse_probe(&payload) else {
            panic!("expected data");
        };
        assert!(data.containers.is_empty());
        assert_eq!(data.images.len(), 1);
    }

    #[test]
    fn every_command_quotes_the_name() {
        let evil = "foo; rm -rf ~";
        for cmd in [
            start_container_command(evil).unwrap(),
            stop_container_command(evil).unwrap(),
            restart_container_command(evil).unwrap(),
            remove_container_command(evil).unwrap(),
        ] {
            assert!(
                !cmd.replace(&format!("'{evil}'"), "").contains("rm -rf"),
                "unquoted: {cmd}"
            );
        }
    }

    #[test]
    fn a_name_with_a_line_break_is_refused() {
        assert!(start_container_command("foo\nbar").is_err());
        assert!(stop_container_command("foo\rbar").is_err());
    }
}
