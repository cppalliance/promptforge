import assert from "node:assert/strict";
import {
  mkdtempSync,
  mkdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  checkIntegrationTestCeilings,
  physicalLineCount,
  repoPath,
} from "./check-integration-test-ceilings.mjs";

function fixture(files, suite = {}, entrySource = 'include!("split/case.rs");\n') {
  const root = mkdtempSync(join(tmpdir(), "promptforge-integration-ceilings-"));
  const suitePath = "crates/demo/tests/it/split";
  const entryPath = "crates/demo/tests/it/split.rs";
  const fixtureFiles = {
    [entryPath]: entrySource,
    ...files,
  };
  for (const [relativePath, source] of Object.entries(fixtureFiles)) {
    const filePath = join(root, ...relativePath.split("/"));
    mkdirSync(join(filePath, ".."), { recursive: true });
    writeFileSync(filePath, source);
  }
  const manifest = {
    version: 1,
    suites: {
      [suitePath]: {
        testTotal: 1,
        entry: {
          path: entryPath,
          ceiling: 1,
        },
        files: {
          "case.rs": 2,
        },
        ...suite,
      },
    },
  };
  const options = { requiredSuites: [suitePath] };
  return { entryPath, manifest, options, root, suitePath };
}

function removeFixture(root) {
  rmSync(root, { force: true, recursive: true });
}

function checkFixture(fixtureState) {
  return checkIntegrationTestCeilings(
    fixtureState.root,
    fixtureState.manifest,
    fixtureState.options,
  );
}

test("normalizes host path separators before manifest comparison", () => {
  assert.equal(
    repoPath(String.raw`crates\gateway\tests\it\realtime_stt\protocol.rs`),
    "crates/gateway/tests/it/realtime_stt/protocol.rs",
  );
});

test("counts physical lines independent of newline convention", () => {
  assert.equal(physicalLineCount("#[test]\nfn case() {}\n"), 2);
  assert.equal(physicalLineCount("#[test]\r\nfn case() {}\r\n"), 2);
  assert.equal(physicalLineCount("#[test]\nfn case() {}"), 2);
});

test("accepts exact file coverage, line ceilings, and test total", () => {
  const fixtureState = fixture({
    "crates/demo/tests/it/split/case.rs": "#[test]\nfn case() {}\n",
  });
  try {
    assert.doesNotThrow(() => checkFixture(fixtureState));
  } finally {
    removeFixture(fixtureState.root);
  }
});

test("fails closed when a manifest file is missing", () => {
  const fixtureState = fixture({});
  try {
    assert.throws(
      () => checkFixture(fixtureState),
      /missing manifested integration test file.*case\.rs/,
    );
  } finally {
    removeFixture(fixtureState.root);
  }
});

test("fails closed when an extra Rust file is discovered", () => {
  const fixtureState = fixture({
    "crates/demo/tests/it/split/case.rs": "#[test]\nfn case() {}\n",
    "crates/demo/tests/it/split/extra.rs": "",
  });
  try {
    assert.throws(
      () => checkFixture(fixtureState),
      /unmanifested integration test file.*extra\.rs/,
    );
  } finally {
    removeFixture(fixtureState.root);
  }
});

test("fails closed when a file exceeds its physical-line ceiling", () => {
  const fixtureState = fixture({
    "crates/demo/tests/it/split/case.rs":
      "#[test]\nfn case() {\n    assert!(true);\n}\n",
  });
  try {
    assert.throws(
      () => checkFixture(fixtureState),
      /case\.rs has 4 physical lines, ceiling is 2/,
    );
  } finally {
    removeFixture(fixtureState.root);
  }
});

test("fails closed when the exact test total drifts", () => {
  const fixtureState = fixture(
    {
      "crates/demo/tests/it/split/case.rs":
        "#[test]\nfn first() {}\n\n#[tokio::test]\nasync fn second() {}\n",
    },
    {
      files: {
        "case.rs": 5,
      },
    },
  );
  try {
    assert.throws(
      () => checkFixture(fixtureState),
      /split has 2 tests, expected exactly 1/,
    );
  } finally {
    removeFixture(fixtureState.root);
  }
});

test("fails closed when manifest paths are not repository-normalized", () => {
  const fixtureState = fixture({
    "crates/demo/tests/it/split/case.rs": "#[test]\nfn case() {}\n",
  });
  fixtureState.manifest.suites = {
    [String.raw`crates\demo\tests\it\split`]:
      fixtureState.manifest.suites[fixtureState.suitePath],
  };
  fixtureState.options.requiredSuites = [String.raw`crates\demo\tests\it\split`];
  try {
    assert.throws(
      () => checkFixture(fixtureState),
      /suite path must use normalized repository separators/,
    );
  } finally {
    removeFixture(fixtureState.root);
  }
});

test("does not count a test attribute inside a block comment", () => {
  const source = [
    "/*",
    "#[test]",
    "fn commented_out() {}",
    "*/",
    "",
  ].join("\n");
  const fixtureState = fixture(
    { "crates/demo/tests/it/split/case.rs": source },
    {
      files: {
        "case.rs": 4,
      },
    },
  );
  try {
    assert.throws(
      () => checkFixture(fixtureState),
      /split has 0 tests, expected exactly 1/,
    );
  } finally {
    removeFixture(fixtureState.root);
  }
});

test("counts a multiline supported test attribute", () => {
  const source = [
    "#[",
    "  tokio::test(",
    '    flavor = "current_thread"',
    "  )",
    "]",
    "async fn case() {}",
    "",
  ].join("\n");
  const fixtureState = fixture(
    { "crates/demo/tests/it/split/case.rs": source },
    {
      files: {
        "case.rs": 6,
      },
    },
  );
  try {
    assert.doesNotThrow(() => checkFixture(fixtureState));
  } finally {
    removeFixture(fixtureState.root);
  }
});

test("fails closed when a cfg-disabled test replaces a discovered test", () => {
  const fixtureState = fixture(
    {
      "crates/demo/tests/it/split/case.rs":
        "#[cfg(any())]\n#[test]\nfn disabled() {}\n",
    },
    {
      files: {
        "case.rs": 3,
      },
    },
  );
  try {
    assert.throws(
      () => checkFixture(fixtureState),
      /cfg-gated test is unsupported.*case\.rs/,
    );
  } finally {
    removeFixture(fixtureState.root);
  }
});

test("fails closed when a macro-contained test replaces a discovered test", () => {
  const source = [
    "macro_rules! generated_test {",
    "  () => {",
    "    #[test]",
    "    fn generated() {}",
    "  };",
    "}",
    "",
  ].join("\n");
  const fixtureState = fixture(
    { "crates/demo/tests/it/split/case.rs": source },
    {
      files: {
        "case.rs": 6,
      },
    },
  );
  try {
    assert.throws(
      () => checkFixture(fixtureState),
      /macro-generated test is unsupported.*case\.rs/,
    );
  } finally {
    removeFixture(fixtureState.root);
  }
});

test("fails closed when a suite entry exceeds its physical-line ceiling", () => {
  const fixtureState = fixture(
    {
      "crates/demo/tests/it/split/case.rs": "#[test]\nfn case() {}\n",
    },
    {},
    'include!("split/case.rs");\n\n',
  );
  try {
    assert.throws(
      () => checkFixture(fixtureState),
      /split\.rs has 2 physical lines, ceiling is 1/,
    );
  } finally {
    removeFixture(fixtureState.root);
  }
});

test("fails closed when a required include is missing", () => {
  const fixtureState = fixture(
    {
      "crates/demo/tests/it/split/case.rs": "#[test]\nfn case() {}\n",
    },
    {},
    "",
  );
  try {
    assert.throws(
      () => checkFixture(fixtureState),
      /suite include coverage differs.*missing.*split\/case\.rs/,
    );
  } finally {
    removeFixture(fixtureState.root);
  }
});

test("fails closed when a required include is replaced", () => {
  const fixtureState = fixture(
    {
      "crates/demo/tests/it/split/case.rs": "#[test]\nfn case() {}\n",
    },
    {},
    'include!("split/replacement.rs");\n',
  );
  try {
    assert.throws(
      () => checkFixture(fixtureState),
      /suite include coverage differs: missing \["split\/case\.rs"\], extra \["split\/replacement\.rs"\]/,
    );
  } finally {
    removeFixture(fixtureState.root);
  }
});

test("fails closed when a required include appears twice", () => {
  const fixtureState = fixture(
    {
      "crates/demo/tests/it/split/case.rs": "#[test]\nfn case() {}\n",
    },
    {
      entry: {
        path: "crates/demo/tests/it/split.rs",
        ceiling: 2,
      },
    },
    'include!("split/case.rs");\ninclude!("split/case.rs");\n',
  );
  try {
    assert.throws(
      () => checkFixture(fixtureState),
      /suite include must appear exactly once.*split\/case\.rs.*found 2/,
    );
  } finally {
    removeFixture(fixtureState.root);
  }
});

test("fails closed when a required suite is omitted", () => {
  const fixtureState = fixture({
    "crates/demo/tests/it/split/case.rs": "#[test]\nfn case() {}\n",
  });
  fixtureState.manifest.suites = {};
  try {
    assert.throws(
      () => checkFixture(fixtureState),
      /suite coverage differs: missing \["crates\/demo\/tests\/it\/split"\], extra \[\]/,
    );
  } finally {
    removeFixture(fixtureState.root);
  }
});

test("fails closed when a required suite is replaced by an undeclared suite", () => {
  const fixtureState = fixture({
    "crates/demo/tests/it/split/case.rs": "#[test]\nfn case() {}\n",
  });
  fixtureState.manifest.suites = {
    "crates/demo/tests/it/replacement":
      fixtureState.manifest.suites[fixtureState.suitePath],
  };
  try {
    assert.throws(
      () => checkFixture(fixtureState),
      /suite coverage differs: missing \["crates\/demo\/tests\/it\/split"\], extra \["crates\/demo\/tests\/it\/replacement"\]/,
    );
  } finally {
    removeFixture(fixtureState.root);
  }
});
