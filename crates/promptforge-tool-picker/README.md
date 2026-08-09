# promptforge-tool-picker

`promptforge-tool-picker` resolves a plain-English capability need against an ordered tool catalog. It owns an embedded sentence model, validated decision policy, borrowing outcomes, shortlists, and selected-scope near-duplicate analysis.

Load [`Model`](https://docs.rs/promptforge-tool-picker/latest/promptforge_tool_picker/struct.Model.html) once when several catalogs need independent pickers. `ToolPicker::build` remains the one-call path.

The default `serde` feature preserves the catalog and configuration wire formats. Disabling default features removes serde trait implementations while retaining `serde_json::Value` as the input-schema type.

Clean builds require an immutable pinned model snapshot supplied through `PROMPTFORGE_MODEL_DIR`. Cargo performs no network access.

This private workspace crate follows the workspace MSRV and BSL-1.0 license.
