use crate::model::ConnectionInput;
use serde::Serialize;
use tokio_postgres::{
    Config,
    config::{Host, SslMode},
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionDefaults {
    pub input: ConnectionInput,
    pub auto_connect: bool,
    pub error: Option<String>,
    #[serde(skip)]
    pub config: Option<Config>,
}

pub fn environment_defaults(get: impl Fn(&str) -> Option<String>) -> ConnectionDefaults {
    let value = |name| get(name).filter(|value| !value.is_empty());
    let mut input = ConnectionInput {
        host: "localhost".into(),
        port: 5432,
        database: "postgres".into(),
        user: "postgres".into(),
        password: "postgres".into(),
        tls: false,
    };
    let result = (|| {
        if let Some(url) = value("DATABASE_URL") {
            let config = parse_database_url(&url)?;
            input = profile(&config)?;
            return Ok(config);
        }
        input.host = value("PGHOST").unwrap_or(input.host.clone());
        input.user = value("PGUSER").unwrap_or(input.user.clone());
        input.database = value("PGDATABASE").unwrap_or(input.user.clone());
        // An explicitly empty password is different from an unset password.
        input.password = get("PGPASSWORD").unwrap_or(input.password.clone());
        if !input.host.starts_with('/') {
            input.port = value("PGPORT")
                .unwrap_or_else(|| "5432".into())
                .parse::<u16>()
                .ok()
                .filter(|port| *port > 0)
                .unwrap_or(0);
            if input.port == 0 {
                return Err("PGPORT must be an integer between 1 and 65535.".into());
            }
        }
        let resolved = resolve(input.clone())?;
        input.port = resolved.port;
        input.tls = resolved.tls;
        let mut config = crate::database::configuration(&resolved);
        if !resolved.host.starts_with('/') {
            config.ssl_mode(SslMode::Prefer);
        }
        Ok(config)
    })();
    let (config, error) = match result {
        Ok(config) => (Some(config), None),
        Err(error) => (None, Some(error)),
    };
    ConnectionDefaults {
        input,
        auto_connect: true,
        error,
        config,
    }
}

fn parse_database_url(url: &str) -> Result<Config, String> {
    const INVALID: &str = "Invalid DATABASE_URL. Use a postgres:// or postgresql:// connection URL with supported PostgreSQL options.";
    if !url.starts_with("postgres://") && !url.starts_with("postgresql://") {
        return Err(INVALID.into());
    }
    // Parser errors may contain supplied values. Never echo a URL or its password.
    let mut config: Config = url.parse().map_err(|_| INVALID.to_string())?;
    if config.get_hosts().is_empty() && config.get_hostaddrs().is_empty() {
        config.host("localhost");
    }
    if config.get_ports().is_empty() {
        config.port(5432);
    }
    if config.get_ports().contains(&0) {
        return Err("DATABASE_URL port must be between 1 and 65535.".into());
    }
    if config.get_user().is_none_or(str::is_empty) {
        config.user("postgres");
    }
    if config.get_password().is_none() {
        config.password("postgres");
    }
    if config.get_dbname().is_none_or(str::is_empty) {
        config.dbname(config.get_user().unwrap().to_owned());
    }
    Ok(config)
}

fn profile(config: &Config) -> Result<ConnectionInput, String> {
    let host = match config.get_hosts().first() {
        Some(Host::Tcp(host)) => host.clone(),
        #[cfg(unix)]
        Some(Host::Unix(path)) => path.to_string_lossy().into_owned(),
        None => config
            .get_hostaddrs()
            .first()
            .map(ToString::to_string)
            .unwrap_or_else(|| "localhost".into()),
    };
    Ok(ConnectionInput {
        host,
        port: config.get_ports().first().copied().unwrap_or(5432),
        database: config.get_dbname().unwrap().into(),
        user: config.get_user().unwrap().into(),
        password: String::from_utf8(config.get_password().unwrap().to_vec())
            .map_err(|_| "DATABASE_URL password must be valid UTF-8.".to_string())?,
        tls: config.get_ssl_mode() == SslMode::Require,
    })
}

/// Freeze the selected socket's directory and port before testing/storing a connection.
pub fn resolve(mut input: ConnectionInput) -> Result<ConnectionInput, String> {
    if input.host.starts_with('/') {
        let (directory, port) = socket_address(&input.host)?;
        input.host = directory;
        input.port = port;
        input.tls = false;
    } else if input.port == 0 {
        return Err("Port must be between 1 and 65535.".into());
    }
    Ok(input)
}

#[cfg(unix)]
fn socket_address(host: &str) -> Result<(String, u16), String> {
    use std::{fs, os::unix::fs::FileTypeExt, path::Path};

    fn socket_port(path: &Path) -> Option<u16> {
        path.file_name()?
            .to_str()?
            .strip_prefix(".s.PGSQL.")?
            .parse::<u16>()
            .ok()
            .filter(|port| *port > 0)
    }

    let path = Path::new(host);
    let metadata = fs::metadata(path)
        .map_err(|error| format!("Could not access PostgreSQL socket path {host}: {error}"))?;
    if metadata.file_type().is_socket() {
        let port = socket_port(path)
            .ok_or("A PostgreSQL socket filename must have the form .s.PGSQL.<port>.")?;
        return Ok((path.parent().unwrap().to_string_lossy().into_owned(), port));
    }
    if !metadata.is_dir() {
        return Err(format!("{host} is not a Unix socket or socket directory."));
    }
    let mut ports = Vec::new();
    for entry in fs::read_dir(path)
        .map_err(|error| format!("Could not read PostgreSQL socket directory {host}: {error}"))?
    {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_socket()
            && let Some(port) = socket_port(&entry.path())
        {
            ports.push(port);
        }
    }
    match ports.as_slice() {
        [port] => Ok((host.into(), *port)),
        [] => Err(format!(
            "No PostgreSQL socket found in {host}. Start PostgreSQL or choose another host."
        )),
        _ => Err(format!(
            "Multiple PostgreSQL sockets found in {host}. Set Host or PGHOST to the full .s.PGSQL.<port> socket path."
        )),
    }
}

#[cfg(not(unix))]
fn socket_address(_host: &str) -> Result<(String, u16), String> {
    Err("Unix socket connections are not supported on this platform.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults(vars: &[(&str, &str)]) -> ConnectionDefaults {
        environment_defaults(|name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).into())
        })
    }

    #[test]
    fn pg_variables_use_explicit_defaults_and_connect_without_host_or_port() {
        let empty = defaults(&[]);
        assert!(empty.auto_connect);
        assert!(empty.error.is_none());
        assert_eq!(empty.input.host, "localhost");
        assert_eq!(empty.input.port, 5432);
        assert_eq!(empty.input.user, "postgres");
        assert_eq!(empty.input.database, "postgres");
        assert_eq!(empty.input.password, "postgres");
        let user_only = defaults(&[("PGUSER", "env_user"), ("PGHOST", "")]);
        assert!(user_only.auto_connect);
        assert_eq!(user_only.input.database, "env_user");
        let database_only = defaults(&[("PGDATABASE", "env_db"), ("PGPASSWORD", "")]);
        assert_eq!(database_only.input.database, "env_db");
        assert_eq!(database_only.input.password, "");

        let tcp = defaults(&[
            ("PGHOST", "db.example"),
            ("PGPORT", "5433"),
            ("PGDATABASE", "env_db"),
            ("PGUSER", "env_user"),
            ("PGPASSWORD", "secret"),
        ]);
        assert!(tcp.auto_connect);
        assert!(tcp.error.is_none());
        assert_eq!(tcp.input.host, "db.example");
        assert_eq!(tcp.input.port, 5433);
        assert_eq!(tcp.input.database, "env_db");
        assert_eq!(tcp.input.user, "env_user");
        assert_eq!(tcp.input.password, "secret");
        assert_eq!(tcp.config.unwrap().get_ssl_mode(), SslMode::Prefer);

        let host_only = defaults(&[("PGHOST", "localhost")]);
        assert_eq!(host_only.input.port, 5432);
        assert_eq!(host_only.input.database, "postgres");
        let port_only = defaults(&[("PGPORT", "5433")]);
        assert_eq!(port_only.input.host, "localhost");
        assert_eq!(port_only.input.port, 5433);
        for port in ["0", "65536", "invalid"] {
            assert!(defaults(&[("PGPORT", port)]).error.is_some());
        }
    }

    #[test]
    fn database_url_takes_precedence_and_preserves_encoding_and_options() {
        let result = defaults(&[
            (
                "DATABASE_URL",
                "postgresql://url%40user:p%40ss%3Aword%2F%25@[::1]:55435/my%20db?sslmode=require&application_name=url-client&options=-c%20search_path%3Dpublic",
            ),
            ("PGHOST", "/does/not/exist"),
            ("PGPORT", "invalid"),
            ("PGUSER", "ignored"),
            ("PGDATABASE", "ignored"),
            ("PGPASSWORD", "ignored"),
        ]);
        assert!(result.error.is_none(), "{:?}", result.error);
        assert_eq!(result.input.host, "::1");
        assert_eq!(result.input.port, 55435);
        assert_eq!(result.input.user, "url@user");
        assert_eq!(result.input.password, "p@ss:word/%");
        assert_eq!(result.input.database, "my db");
        assert!(result.input.tls);
        let config = result.config.unwrap();
        assert_eq!(config.get_application_name(), Some("url-client"));
        assert_eq!(config.get_options(), Some("-c search_path=public"));

        let partial = defaults(&[
            ("DATABASE_URL", "postgres://url_user@db.example/"),
            ("PGPASSWORD", "ignored"),
        ]);
        assert_eq!(partial.input.database, "url_user");
        assert_eq!(partial.input.password, "postgres");
        assert_eq!(partial.input.port, 5432);
        let blank = defaults(&[("DATABASE_URL", ""), ("PGUSER", "fallback")]);
        assert_eq!(blank.input.user, "fallback");
        let empty_password = defaults(&[("DATABASE_URL", "postgres://postgres:@localhost/")]);
        assert_eq!(empty_password.input.password, "");
    }

    #[test]
    fn invalid_database_url_does_not_fall_back_or_disclose_credentials() {
        for url in [
            "https://user:keep-this-secret@host/database",
            "postgres://user:keep-this-secret@host:invalid/database",
            "postgres://user:keep-this-secret@host/database?unknown=keep-this-secret",
            "postgres://user:keep-this-secret@host:0/database",
        ] {
            let result = defaults(&[("DATABASE_URL", url), ("PGHOST", "fallback.example")]);
            assert!(result.config.is_none());
            let error = result.error.unwrap();
            assert!(error.contains("DATABASE_URL"));
            assert!(!error.contains("keep-this-secret"));
            assert_ne!(result.input.host, "fallback.example");
        }
    }

    #[cfg(unix)]
    #[test]
    fn sockets_ignore_pgport_and_require_an_unambiguous_target() {
        use std::{fs, os::unix::net::UnixListener};
        let root = std::env::temp_dir().join(format!("jupsql-sockets-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let host = root.to_str().unwrap();
        // Lock files and ordinary files with socket-like names are not candidates.
        fs::write(root.join(".s.PGSQL.5432.lock"), "").unwrap();
        fs::write(root.join(".s.PGSQL.5432"), "").unwrap();
        assert!(
            defaults(&[("PGHOST", host)])
                .error
                .unwrap()
                .contains("No PostgreSQL socket")
        );
        let path = root.join(".s.PGSQL.55433");
        let _socket = UnixListener::bind(&path).unwrap();
        for vars in [
            vec![("PGHOST", host)],
            vec![("PGHOST", host), ("PGPORT", "invalid")],
        ] {
            let result = defaults(&vars);
            assert!(result.error.is_none());
            assert_eq!(result.input.port, 55433);
            assert!(!result.input.tls);
        }
        let _other = UnixListener::bind(root.join(".s.PGSQL.55434")).unwrap();
        assert!(
            defaults(&[("PGHOST", host), ("PGPORT", "55433")])
                .error
                .unwrap()
                .contains("Multiple PostgreSQL sockets")
        );
        let explicit = defaults(&[("PGHOST", path.to_str().unwrap()), ("PGPORT", "invalid")]);
        assert!(explicit.error.is_none());
        assert_eq!(explicit.input.host, path.to_str().unwrap());
        let resolved = resolve(explicit.input).unwrap();
        assert_eq!(resolved.host, host);
        assert_eq!(resolved.port, 55433);
        fs::remove_dir_all(root).unwrap();
    }
}
