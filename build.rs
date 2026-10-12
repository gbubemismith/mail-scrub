fn main() {
    // sqlx::migrate! embeds the migration files at compile time.
    println!("cargo:rerun-if-changed=migrations");
}
