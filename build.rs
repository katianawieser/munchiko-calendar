// Rebuild when a migration is added or changed. The migrations are embedded
// into the binary by `sqlx::migrate!()`, and without this hint Cargo wouldn't
// notice that a new .sql file needs a recompile.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
