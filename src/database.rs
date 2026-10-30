use std::ffi::OsString;
use std::fs::{self, File, FileType};
use std::io::Read;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::scan::{is_dependency_dir, is_package};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    Postgres,
    Mysql,
    ClickHouse,
    Sqlite,
}

impl Engine {
    pub fn name(self) -> &'static str {
        match self {
            Engine::Postgres => "PostgreSQL",
            Engine::Mysql => "MySQL",
            Engine::ClickHouse => "ClickHouse",
            Engine::Sqlite => "SQLite",
        }
    }
}

const SQLITE_EXTENSIONS: &[&str] = &["sqlite", "sqlite3", "db", "db3", "s3db", "sl3"];
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

/// The database that owns this data folder or file.
pub fn engine(path: &Path) -> Option<Engine> {
    if path.is_file() {
        return is_sqlite(path).then_some(Engine::Sqlite);
    }

    let names: Vec<OsString> = fs::read_dir(path)
        .ok()?
        .filter_map(|entry| entry.ok().map(|e| e.file_name()))
        .collect();
    data_dir_engine(|name| names.iter().any(|n| n == name))
}

/// Data folders of PostgreSQL, MySQL and ClickHouse, and SQLite files.
pub fn find(dir: &Path, skip: &[PathBuf], depth: u8) -> Vec<PathBuf> {
    if depth == 0 {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };

    let entries: Vec<(PathBuf, Option<FileType>)> = entries
        .filter_map(Result::ok)
        .map(|entry| (entry.path(), entry.file_type().ok()))
        .collect();

    let is_data_dir = data_dir_engine(|name| {
        entries
            .iter()
            .any(|(path, _)| path.file_name().is_some_and(|n| n == name))
    })
    .is_some();
    if is_data_dir {
        return vec![dir.to_path_buf()];
    }

    entries
        .par_iter()
        .flat_map(|(path, kind)| match kind {
            Some(kind) if kind.is_dir() && !should_skip(path, skip) => find(path, skip, depth - 1),
            Some(kind) if kind.is_file() && is_sqlite(path) => vec![path.clone()],
            _ => Vec::new(),
        })
        .collect()
}

fn data_dir_engine(has: impl Fn(&str) -> bool) -> Option<Engine> {
    if has("PG_VERSION") && has("global") {
        Some(Engine::Postgres)
    } else if has("ibdata1") || has("mysql.ibd") {
        Some(Engine::Mysql)
    } else if has("metadata") && has("uuid") {
        Some(Engine::ClickHouse)
    } else {
        None
    }
}

fn is_sqlite(path: &Path) -> bool {
    let known = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| SQLITE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()));
    if !known {
        return false;
    }

    let mut header = [0u8; 16];
    File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .is_ok_and(|()| &header == SQLITE_HEADER)
}

fn should_skip(path: &Path, skip: &[PathBuf]) -> bool {
    path.file_name().is_some_and(|name| name == ".git")
        || is_dependency_dir(path)
        || is_package(path)
        || skip.iter().any(|dir| dir == path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path, content: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn sqlite() -> Vec<u8> {
        [SQLITE_HEADER.as_slice(), &[0u8; 100]].concat()
    }

    #[test]
    fn find_should_find_data_folders_and_sqlite_files() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("api/.docker/pgdata/PG_VERSION"), b"17");
        fs::create_dir_all(dir.path().join("api/.docker/pgdata/global")).unwrap();
        touch(&dir.path().join("shop/mysql/ibdata1"), b"x");
        touch(&dir.path().join("stats/clickhouse/uuid"), b"x");
        fs::create_dir_all(dir.path().join("stats/clickhouse/metadata")).unwrap();
        touch(&dir.path().join("blog/database/database.sqlite"), &sqlite());

        let mut found = find(dir.path(), &[], 8);
        found.sort();

        let expected = [
            "api/.docker/pgdata",
            "blog/database/database.sqlite",
            "shop/mysql",
            "stats/clickhouse",
        ]
        .map(|p| dir.path().join(p));
        assert_eq!(found, expected);
    }

    #[test]
    fn find_should_ignore_db_files_that_are_not_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("app/cache.db"), b"not a database");

        assert!(find(dir.path(), &[], 5).is_empty());
    }

    #[test]
    fn find_should_not_enter_dependency_and_git_folders() {
        let dir = tempfile::tempdir().unwrap();
        touch(
            &dir.path().join("web/node_modules/x/test.sqlite"),
            &sqlite(),
        );
        touch(&dir.path().join("web/.git/x.sqlite"), &sqlite());

        assert!(find(dir.path(), &[], 5).is_empty());
    }

    #[test]
    fn engine_should_name_data_folder() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("PG_VERSION"), b"17");
        fs::create_dir_all(dir.path().join("global")).unwrap();

        assert_eq!(engine(dir.path()), Some(Engine::Postgres));
    }
}
