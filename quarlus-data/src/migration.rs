//! Database migration helpers.
//!
//! SQLx uses a compile-time macro `sqlx::migrate!()` that embeds migrations
//! from the `./migrations` directory relative to `CARGO_MANIFEST_DIR`. Because
//! this macro must be invoked in the user's crate (not a library), Quarlus
//! does not wrap it—instead, use it directly in your startup hook.
//!
//! # Setup
//!
//! 1. Create a `migrations/` directory in your project root.
//! 2. Add `.sql` files with a timestamp prefix (use `sqlx migrate add <name>`).
//! 3. Run migrations on startup:
//!
//! ```ignore
//! AppBuilder::new()
//!     .on_start(|state| async move {
//!         sqlx::migrate!()
//!             .run(&state.pool)
//!             .await
//!             .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
//!         Ok(())
//!     })
//!     .serve("0.0.0.0:3000")
//!     .await?;
//! ```
//!
//! # Reverting
//!
//! Use the `sqlx` CLI:
//!
//! ```bash
//! sqlx migrate revert
//! ```
