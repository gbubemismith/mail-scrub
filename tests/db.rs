use mail_scrub::db::Db;
use sqlx::Row;
use sqlx::sqlite::SqlitePool;

#[tokio::test]
async fn opening_creates_the_file_with_every_table_and_is_repeatable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("mailscrub.db");

    Db::open(&path).await.unwrap();
    // A second open must not fail or recreate anything.
    Db::open(&path).await.unwrap();

    let pool = SqlitePool::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    let tables: Vec<String> = sqlx::query(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '\\_sqlx%' ESCAPE '\\' ORDER BY name",
    )
    .fetch_all(&pool)
    .await
    .unwrap()
    .iter()
    .map(|r| r.get("name"))
    .collect();
    assert_eq!(
        tables,
        [
            "decision",
            "domain_cache",
            "filter",
            "message",
            "pin",
            "run"
        ]
    );
}
