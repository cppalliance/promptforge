//! Bounded GGUF header parser tests.

use super::*;

/// Appends a GGUF string (u64 LE length + bytes) to `out`.
fn push_string(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&(value.len() as u64).to_le_bytes());
    out.extend_from_slice(value.as_bytes());
}

/// The fixed GGUF prelude: magic, version 3, and the two counts.
fn push_prelude(out: &mut Vec<u8>, tensor_count: u64, metadata_count: u64) {
    out.extend_from_slice(b"GGUF");
    out.extend_from_slice(&3u32.to_le_bytes());
    out.extend_from_slice(&tensor_count.to_le_bytes());
    out.extend_from_slice(&metadata_count.to_le_bytes());
}

/// A minimal well-formed GGUF header: architecture, block count,
/// declared parameter count, plus one skipped string array and one
/// skipped float to exercise the skip paths.
fn synthetic_gguf() -> Vec<u8> {
    let mut out = Vec::new();
    push_prelude(&mut out, 0, 6);
    push_string(&mut out, "general.architecture");
    out.extend_from_slice(&TYPE_STRING.to_le_bytes());
    push_string(&mut out, "llama");
    push_string(&mut out, "tokenizer.ggml.tokens");
    out.extend_from_slice(&TYPE_ARRAY.to_le_bytes());
    out.extend_from_slice(&TYPE_STRING.to_le_bytes());
    out.extend_from_slice(&2u64.to_le_bytes());
    push_string(&mut out, "<s>");
    push_string(&mut out, "</s>");
    push_string(&mut out, "llama.block_count");
    out.extend_from_slice(&TYPE_UINT32.to_le_bytes());
    out.extend_from_slice(&32u32.to_le_bytes());
    push_string(&mut out, "general.parameter_count");
    out.extend_from_slice(&TYPE_UINT64.to_le_bytes());
    out.extend_from_slice(&8_030_000_000u64.to_le_bytes());
    push_string(&mut out, "tokenizer.chat_template");
    out.extend_from_slice(&TYPE_STRING.to_le_bytes());
    push_string(&mut out, "{{ messages }}");
    push_string(&mut out, "llama.rope.freq_base");
    out.extend_from_slice(&TYPE_FLOAT32.to_le_bytes());
    out.extend_from_slice(&10_000.0f32.to_le_bytes());
    out
}

/// Writes `bytes` as `models/<name>` under a fresh cache root and
/// returns the root.
fn cache_with(name: &str, bytes: &[u8]) -> tempfile::TempDir {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let models = temp.path().join("models");
    std::fs::create_dir_all(&models).expect("mkdir models");
    std::fs::write(models.join(name), bytes).expect("write fixture");
    temp
}

#[test]
fn reads_architecture_layer_and_parameter_counts() {
    let root = cache_with("tiny.gguf", &synthetic_gguf());
    let info = read_model_info(root.path(), Path::new("models/tiny.gguf"))
        .expect("the synthetic header parses");
    assert_eq!(
        info,
        ModelInfo {
            architecture: Some("llama".to_owned()),
            layer_count: Some(32),
            parameter_count: Some(8_030_000_000),
            chat_template: Some("{{ messages }}".to_owned()),
        }
    );
}

#[test]
fn parameter_count_falls_back_to_tensor_shapes() {
    let mut bytes = Vec::new();
    push_prelude(&mut bytes, 2, 2);
    push_string(&mut bytes, "general.architecture");
    bytes.extend_from_slice(&TYPE_STRING.to_le_bytes());
    push_string(&mut bytes, "llama");
    push_string(&mut bytes, "llama.block_count");
    bytes.extend_from_slice(&TYPE_UINT32.to_le_bytes());
    bytes.extend_from_slice(&2u32.to_le_bytes());
    // Tensor infos: name, n_dims, dims, ggml type, offset.
    push_string(&mut bytes, "blk.0.attn_q.weight");
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&4_096u64.to_le_bytes());
    bytes.extend_from_slice(&32u64.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    push_string(&mut bytes, "output.bias");
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&10u64.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());

    let root = cache_with("shapes.gguf", &bytes);
    let info = read_model_info(root.path(), Path::new("models/shapes.gguf"))
        .expect("the tensor-shape header parses");
    assert_eq!(info.parameter_count, Some(4_096 * 32 + 10));
    assert_eq!(info.layer_count, Some(2));
    assert_eq!(info.chat_template, None);
}

#[test]
fn oversized_chat_template_is_refused_before_allocation() {
    let mut bytes = Vec::new();
    push_prelude(&mut bytes, 0, 1);
    push_string(&mut bytes, "tokenizer.chat_template");
    bytes.extend_from_slice(&TYPE_STRING.to_le_bytes());
    bytes.extend_from_slice(&(MAX_CHAT_TEMPLATE_BYTES + 1).to_le_bytes());
    let root = cache_with("oversized-template.gguf", &bytes);
    let error = read_model_info(root.path(), Path::new("models/oversized-template.gguf"))
        .expect_err("an oversized chat template is refused");
    assert!(error.to_string().contains("chat template length"));
}

#[test]
fn wrong_magic_is_malformed_not_a_panic() {
    let root = cache_with("junk.gguf", b"not a gguf file at all");
    let error = read_model_info(root.path(), Path::new("models/junk.gguf"))
        .expect_err("junk bytes are refused");
    assert!(matches!(error, LocalError::MalformedGguf { .. }));
    assert!(error.to_string().contains("bad magic"));
}

#[test]
fn truncated_file_is_malformed_not_a_panic() {
    let full = synthetic_gguf();
    let root = cache_with("cut.gguf", &full[..full.len() / 2]);
    let error = read_model_info(root.path(), Path::new("models/cut.gguf"))
        .expect_err("a truncated header is refused");
    assert!(matches!(error, LocalError::MalformedGguf { .. }));
}

#[test]
fn unsupported_version_is_refused() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"GGUF");
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&[0u8; 16]);
    let root = cache_with("v1.gguf", &bytes);
    let error = read_model_info(root.path(), Path::new("models/v1.gguf"))
        .expect_err("version 1 is refused");
    assert!(error.to_string().contains("unsupported GGUF version 1"));
}

#[test]
fn bogus_string_length_is_malformed_not_an_unbounded_read() {
    let mut bytes = Vec::new();
    push_prelude(&mut bytes, 0, 1);
    // A key whose declared length dwarfs the file.
    bytes.extend_from_slice(&u64::MAX.to_le_bytes());
    let root = cache_with("bogus.gguf", &bytes);
    let error = read_model_info(root.path(), Path::new("models/bogus.gguf"))
        .expect_err("a bogus length is refused");
    assert!(matches!(error, LocalError::MalformedGguf { .. }));
}

#[test]
fn entry_count_caps_are_enforced() {
    let mut bytes = Vec::new();
    push_prelude(&mut bytes, 0, MAX_METADATA_ENTRIES + 1);
    let root = cache_with("meta.gguf", &bytes);
    let error = read_model_info(root.path(), Path::new("models/meta.gguf"))
        .expect_err("an oversized metadata count is refused");
    assert!(error.to_string().contains("metadata count"));

    let mut bytes = Vec::new();
    push_prelude(&mut bytes, MAX_TENSORS + 1, 0);
    let root = cache_with("tensors.gguf", &bytes);
    let error = read_model_info(root.path(), Path::new("models/tensors.gguf"))
        .expect_err("an oversized tensor count is refused");
    assert!(error.to_string().contains("tensor count"));
}

#[test]
fn deep_array_nesting_is_refused_not_a_stack_overflow() {
    let mut bytes = Vec::new();
    push_prelude(&mut bytes, 0, 1);
    push_string(&mut bytes, "some.key");
    bytes.extend_from_slice(&TYPE_ARRAY.to_le_bytes());
    // Each nested level is an element type plus a count of one.
    for _ in 0..MAX_ARRAY_DEPTH {
        bytes.extend_from_slice(&TYPE_ARRAY.to_le_bytes());
        bytes.extend_from_slice(&1u64.to_le_bytes());
    }
    let root = cache_with("deep.gguf", &bytes);
    let error = read_model_info(root.path(), Path::new("models/deep.gguf"))
        .expect_err("nesting past the cap is refused");
    assert!(error.to_string().contains("array nesting exceeds the cap"));
}

#[test]
fn array_size_overflow_is_refused() {
    let mut bytes = Vec::new();
    push_prelude(&mut bytes, 0, 1);
    push_string(&mut bytes, "some.key");
    bytes.extend_from_slice(&TYPE_ARRAY.to_le_bytes());
    bytes.extend_from_slice(&TYPE_UINT64.to_le_bytes());
    bytes.extend_from_slice(&u64::MAX.to_le_bytes());
    let root = cache_with("overflow.gguf", &bytes);
    let error = read_model_info(root.path(), Path::new("models/overflow.gguf"))
        .expect_err("an overflowing array byte size is refused");
    assert!(error.to_string().contains("array size overflows"));
}

#[test]
fn skipped_value_overrunning_the_file_is_refused() {
    let mut bytes = Vec::new();
    push_prelude(&mut bytes, 0, 1);
    push_string(&mut bytes, "some.key");
    bytes.extend_from_slice(&TYPE_STRING.to_le_bytes());
    // A skipped string may exceed the materialization cap, but never
    // the file itself.
    bytes.extend_from_slice(&1_000_000u64.to_le_bytes());
    let root = cache_with("overrun.gguf", &bytes);
    let error = read_model_info(root.path(), Path::new("models/overrun.gguf"))
        .expect_err("a skip past end-of-file is refused");
    assert!(error.to_string().contains("value overruns the file"));
}

/// One tensor-info record: name, the given dims, ggml type, offset.
fn push_tensor(out: &mut Vec<u8>, dims: &[u64]) {
    push_string(out, "t");
    let dimension_count = u32::try_from(dims.len()).expect("test dims fit u32");
    out.extend_from_slice(&dimension_count.to_le_bytes());
    for dim in dims {
        out.extend_from_slice(&dim.to_le_bytes());
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes());
}

#[test]
fn tensor_dimension_cap_is_enforced() {
    let mut bytes = Vec::new();
    push_prelude(&mut bytes, 1, 0);
    push_string(&mut bytes, "t");
    bytes.extend_from_slice(&(MAX_TENSOR_DIMS + 1).to_le_bytes());
    let root = cache_with("dims.gguf", &bytes);
    let error = read_model_info(root.path(), Path::new("models/dims.gguf"))
        .expect_err("an oversized dimension count is refused");
    assert!(error.to_string().contains("tensor dimension count"));
}

#[test]
fn tensor_shape_overflow_is_refused() {
    let mut bytes = Vec::new();
    push_prelude(&mut bytes, 1, 0);
    // Three maximal dims overflow even the u128 accumulator.
    push_tensor(&mut bytes, &[u64::MAX, u64::MAX, u64::MAX]);
    let root = cache_with("shape.gguf", &bytes);
    let error = read_model_info(root.path(), Path::new("models/shape.gguf"))
        .expect_err("an overflowing tensor shape is refused");
    assert!(error.to_string().contains("tensor shape overflows"));
}

#[test]
fn parameter_count_past_u64_is_refused() {
    let mut bytes = Vec::new();
    push_prelude(&mut bytes, 1, 0);
    // The element count fits u128 but not the reported u64.
    push_tensor(&mut bytes, &[u64::MAX, 2]);
    let root = cache_with("params.gguf", &bytes);
    let error = read_model_info(root.path(), Path::new("models/params.gguf"))
        .expect_err("a parameter count past u64 is refused");
    assert!(error.to_string().contains("parameter count overflows"));
}

#[test]
fn traversal_and_absolute_paths_are_refused() {
    let root = cache_with("tiny.gguf", &synthetic_gguf());
    for escape in [
        Path::new("../outside.gguf"),
        Path::new("models/../../outside.gguf"),
    ] {
        assert!(matches!(
            read_model_info(root.path(), escape),
            Err(LocalError::UnsafeCachePath { .. })
        ));
    }
    let absolute = root.path().join("models").join("tiny.gguf");
    assert!(
        matches!(
            read_model_info(root.path(), &absolute),
            Err(LocalError::UnsafeCachePath { .. })
        ),
        "an absolute path is refused even when it points inside the cache"
    );
}
