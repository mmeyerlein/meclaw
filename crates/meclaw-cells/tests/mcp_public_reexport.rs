//! T22: verify `McpCellFactory` is re-exported from the crate root.

#[test]
fn mcp_cell_factory_is_reexported_at_crate_root() {
    // Compile-only — touching the re-exported type proves the `pub use` exists.
    let _: fn() -> bool = || {
        fn _check<T>() {}
        _check::<meclaw_cells::McpCellFactory>();
        true
    };
}
