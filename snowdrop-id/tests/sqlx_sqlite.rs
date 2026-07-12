//! Round-trip integration test for the sqlx `Type`/`Encode`/`Decode` impls,
//! using an in-memory SQLite database. Runs with `--features sqlx-sqlite`.

use snowdrop_id::{Id, IdGenerator, MachineId};
use sqlx::{Row, SqlitePool};

#[tokio::test]
async fn snowdrop_id_roundtrips_through_bigint_column() {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query("CREATE TABLE posts (id BIGINT PRIMARY KEY, title TEXT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();

    let generator = IdGenerator::new(MachineId::new(9).unwrap());
    let mut ids = Vec::new();
    for n in 0..100 {
        let id = generator.generate().unwrap();
        sqlx::query("INSERT INTO posts (id, title) VALUES (?, ?)")
            .bind(id)
            .bind(format!("post {n}"))
            .execute(&pool)
            .await
            .unwrap();
        ids.push(id);
    }

    // Typed decode straight back into Id, ordered by the BIGINT
    // column — which must equal generation order.
    let rows = sqlx::query("SELECT id FROM posts ORDER BY id")
        .fetch_all(&pool)
        .await
        .unwrap();
    let fetched: Vec<Id> = rows.iter().map(|r| r.get("id")).collect();
    assert_eq!(fetched, ids);

    // Point lookup binding a Id parameter.
    let title: String = sqlx::query("SELECT title FROM posts WHERE id = ?")
        .bind(ids[42])
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("title");
    assert_eq!(title, "post 42");

    // The stored representation is the plain i64, interchangeable with raw
    // integer reads.
    let raw: i64 = sqlx::query("SELECT id FROM posts ORDER BY id LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("id");
    assert_eq!(raw, ids[0].as_i64());

    // Negative BIGINTs are rejected on decode rather than silently wrapped.
    sqlx::query("INSERT INTO posts (id, title) VALUES (-1, 'bad')")
        .execute(&pool)
        .await
        .unwrap();
    let result = sqlx::query_scalar::<_, Id>("SELECT id FROM posts WHERE id < 0")
        .fetch_one(&pool)
        .await;
    assert!(result.is_err());
}
