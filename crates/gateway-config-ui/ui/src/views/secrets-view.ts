// The Secrets view: single global .env file management. It lists
// variables as masked password rows with
// per-row reveal and delete, an Add Variable row, and a Save that
// stages the section as its own `.env.next` shadow via PUT /admin/env.
// The global section carries the dedicated HF Token card [Adapted:
// Unsloth] with show/hide and a Test Connection probe through the
// gateway's HF proxy. `${VAR}` cross-references [INVENTED] annotate
// rows the pending config chain points at - computed server-side and
// carried in the GET /admin/env reply, because the config views arrive
// interpolated with secrets redacted. Values arrive in plaintext (the
// route is loopback-and-bearer-guarded); the view masks them by default
// and never logs them.

import { Eye, EyeOff, Trash2, createElement as lucideElement } from "lucide";

import { fileName } from "../format";
import { HfAuthError, UnauthorizedError } from "../services/gateway-api";
import type { EnvFiles, EnvScope, GatewayApi } from "../services/gateway-api";
import type { ConfigStore } from "../services/config-store";
import type { ToastStack } from "shared-ui/toast";

/** Construction dependencies for the Secrets view. */
export interface SecretsViewDeps {
  /** The config store, for the dirty refresh after a save. */
  store: ConfigStore;
  /** The admin API: env read/stage and the HF connectivity probe. */
  api: GatewayApi;
  /** Outcome surfacing. */
  toasts: ToastStack;
}

/** The mounted view. */
export interface SecretsView {
  /** Renders the view into `main`. */
  mount(main: HTMLElement): () => void;
}

/** One editable variable row. */
interface EnvRow {
  key: string;
  value: string;
}

/** The dedicated Hugging Face token variable. */
const HF_KEY = "HF_TOKEN";

/** The dotenv-safe variable name shape the gateway accepts. */
const KEY_PATTERN = /^[A-Za-z_][A-Za-z0-9_]*$/;

/** Builds the Secrets view (fetches fresh env state on every mount). */
export function createSecretsView(deps: SecretsViewDeps): SecretsView {
  const { store, api, toasts } = deps;

  /** Working rows per scope, rebuilt from the gateway on each mount. */
  const rows = new Map<EnvScope, EnvRow[]>();
  /** Row keys whose value input is currently revealed, `scope:key`. */
  const revealed = new Set<string>();
  let env: EnvFiles | null = null;
  let hfStatus = "";
  let main: HTMLElement | null = null;
  let loadController: AbortController | null = null;
  let probeController: AbortController | null = null;

  const scopeRows = (scope: EnvScope): EnvRow[] => {
    let list = rows.get(scope);
    if (!list) {
      list = [];
      rows.set(scope, list);
    }
    return list;
  };

  /** The HF token row of the global scope, when present. */
  const hfRow = (): EnvRow | undefined =>
    scopeRows("global").find((row) => row.key === HF_KEY);

  const load = async (signal: AbortSignal): Promise<void> => {
    env = await api.getEnv(signal);
    revealed.clear();
    hfStatus = "";
    rows.set(
      "global",
      Object.entries(env.global?.vars ?? {}).map(([key, value]) => ({ key, value })),
    );
  };

  /** One masked value input with its reveal toggle. */
  const valueField = (scope: EnvScope, row: EnvRow): HTMLElement => {
    const wrap = document.createElement("span");
    wrap.className = "env-value-wrap";
    const id = `env-${scope}-${row.key}`;
    const label = document.createElement("label");
    label.className = "visually-hidden";
    label.htmlFor = id;
    label.textContent = `Value of ${row.key}`;
    const revealKey = `${scope}:${row.key}`;
    const input = document.createElement("input");
    input.type = revealed.has(revealKey) ? "text" : "password";
    input.id = id;
    input.className = "input env-value";
    input.autocomplete = "off";
    input.value = row.value;
    input.addEventListener("input", () => {
      row.value = input.value;
    });
    const toggle = document.createElement("button");
    toggle.type = "button";
    toggle.className = "button button-xs button-outline reveal-toggle";
    const paint = (): void => {
      const shown = revealed.has(revealKey);
      toggle.setAttribute("aria-label", shown ? `Hide ${row.key}` : `Show ${row.key}`);
      toggle.setAttribute("aria-pressed", String(shown));
      toggle.replaceChildren(
        lucideElement(shown ? EyeOff : Eye, { "aria-hidden": "true", width: 14, height: 14 }),
      );
    };
    toggle.addEventListener("click", () => {
      if (revealed.has(revealKey)) {
        revealed.delete(revealKey);
      } else {
        revealed.add(revealKey);
      }
      input.type = revealed.has(revealKey) ? "text" : "password";
      paint();
    });
    paint();
    wrap.append(label, input, toggle);
    return wrap;
  };

  /** The "used by" annotation for one variable, when the config references it. */
  const usedByNote = (references: Record<string, string[]>, key: string): HTMLElement | null => {
    const labels = references[key];
    if (!labels || labels.length === 0) {
      return null;
    }
    const note = document.createElement("span");
    note.className = "env-used-by";
    note.textContent = `used by: ${labels.join("; ")}`;
    return note;
  };

  /** The Hugging Face card: dedicated token field, show/hide, Test Connection. */
  const hfCard = (renderSection: () => void): HTMLElement => {
    const card = document.createElement("div");
    card.className = "hf-card";
    const heading = document.createElement("h3");
    heading.className = "section-heading";
    heading.textContent = "Hugging Face";
    const row = document.createElement("div");
    row.className = "env-row hf-row";
    const key = document.createElement("span");
    key.className = "env-key";
    key.textContent = HF_KEY;
    let tokenRow = hfRow();
    if (!tokenRow) {
      // A placeholder row: typing into it creates the variable on save.
      tokenRow = { key: HF_KEY, value: "" };
    }
    const field = valueField("global", tokenRow);
    field.querySelector("input")?.addEventListener("input", () => {
      // The placeholder joins the working rows on first input, and the
      // just-created variable becomes deletable without a re-render.
      if (tokenRow && !scopeRows("global").includes(tokenRow)) {
        scopeRows("global").unshift(tokenRow);
      }
      remove.disabled = false;
    });

    const test = document.createElement("button");
    test.type = "button";
    test.className = "button button-xs button-outline hf-test";
    test.textContent = "Test Connection";
    const status = document.createElement("span");
    status.className = "hf-status";
    status.setAttribute("role", "status");
    status.textContent = hfStatus;
    test.addEventListener("click", () => {
      if ((tokenRow?.value ?? "") === "") {
        hfStatus = "Not set";
        status.textContent = hfStatus;
        return;
      }
      hfStatus = "Testing\u2026";
      status.textContent = hfStatus;
      // The probe rides the gateway's HF proxy (there is no whoami
      // route), so it tests the token the running gateway holds - a
      // staged edit counts only after apply plus restart or switch.
      probeController?.abort();
      const controller = new AbortController();
      probeController = controller;
      void api
        .hfSearch([
          ["q", "gguf"],
          ["limit", "1"],
        ], controller.signal)
        .then(() => {
          hfStatus = "Valid";
          status.textContent = hfStatus;
        })
        .catch((error: unknown) => {
          if (error instanceof HfAuthError) {
            hfStatus = "Invalid";
          } else if (error instanceof UnauthorizedError) {
            hfStatus = "";
          } else if (
            error !== null &&
            typeof error === "object" &&
            "name" in error &&
            error.name === "AbortError"
          ) {
            return;
          } else {
            hfStatus = "Connection failed";
            toasts.show(
              error instanceof Error ? error.message : "The connection test failed",
              "error",
            );
          }
          status.textContent = hfStatus;
        })
        .finally(() => {
          if (probeController === controller) {
            probeController = null;
          }
        });
    });

    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "button button-xs button-danger env-delete";
    remove.setAttribute("aria-label", `Delete ${HF_KEY}`);
    remove.append(lucideElement(Trash2, { "aria-hidden": "true", width: 14, height: 14 }));
    remove.disabled = !scopeRows("global").includes(tokenRow);
    remove.addEventListener("click", () => {
      rows.set(
        "global",
        scopeRows("global").filter((entry) => entry.key !== HF_KEY),
      );
      renderSection();
    });

    row.append(key, field, test, status, remove);
    const help = document.createElement("p");
    help.className = "field-help";
    help.textContent =
      "Gates Hugging Face search and downloads. The test probes the token the gateway is running with.";
    card.append(heading, row, help);
    return card;
  };

  /** The body of one env section: rows, Add Variable, Save, note. */
  const sectionBody = (scope: EnvScope, renderSection: () => void): HTMLElement => {
    const body = document.createElement("div");
    body.className = "env-body";
    const references = env?.references ?? {};

    if (scope === "global") {
      body.append(hfCard(renderSection));
    }

    const heading = document.createElement("h3");
    heading.className = "section-heading";
    heading.textContent = "Environment Variables";
    body.append(heading);

    const list = document.createElement("ul");
    list.className = "env-list";
    const listed = scopeRows(scope).filter(
      (row) => !(scope === "global" && row.key === HF_KEY),
    );
    for (const row of listed) {
      const item = document.createElement("li");
      item.className = "env-row";
      item.dataset["key"] = row.key;
      const key = document.createElement("span");
      key.className = "env-key";
      key.textContent = row.key;
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "button button-xs button-danger env-delete";
      remove.setAttribute("aria-label", `Delete ${row.key}`);
      remove.append(lucideElement(Trash2, { "aria-hidden": "true", width: 14, height: 14 }));
      remove.addEventListener("click", () => {
        rows.set(
          scope,
          scopeRows(scope).filter((entry) => entry !== row),
        );
        renderSection();
      });
      item.append(key, valueField(scope, row), remove);
      const usedBy = usedByNote(references, row.key);
      if (usedBy) {
        item.append(usedBy);
      }
      list.append(item);
    }
    if (listed.length === 0) {
      const empty = document.createElement("p");
      empty.className = "view-empty";
      empty.textContent = "No variables.";
      body.append(empty);
    }
    body.append(list);

    const add = document.createElement("div");
    add.className = "env-add-row";
    const keyLabel = document.createElement("label");
    keyLabel.className = "visually-hidden";
    keyLabel.htmlFor = `env-add-key-${scope}`;
    keyLabel.textContent = "New variable name";
    const keyInput = document.createElement("input");
    keyInput.type = "text";
    keyInput.id = `env-add-key-${scope}`;
    keyInput.className = "input env-add-key";
    keyInput.placeholder = "NAME";
    keyInput.autocomplete = "off";
    const valueLabel = document.createElement("label");
    valueLabel.className = "visually-hidden";
    valueLabel.htmlFor = `env-add-value-${scope}`;
    valueLabel.textContent = "New variable value";
    const valueInput = document.createElement("input");
    valueInput.type = "password";
    valueInput.id = `env-add-value-${scope}`;
    valueInput.className = "input env-add-value";
    valueInput.placeholder = "value";
    valueInput.autocomplete = "off";
    const addButton = document.createElement("button");
    addButton.type = "button";
    addButton.className = "button button-xs button-outline env-add";
    addButton.textContent = "Add Variable";
    addButton.addEventListener("click", () => {
      const name = keyInput.value.trim();
      if (!KEY_PATTERN.test(name)) {
        toasts.show(
          "Variable names use letters, digits, and underscores, not starting with a digit",
          "error",
        );
        return;
      }
      if (scopeRows(scope).some((row) => row.key === name)) {
        toasts.show(`${name} already exists in this file`, "error");
        return;
      }
      scopeRows(scope).push({ key: name, value: valueInput.value });
      renderSection();
    });
    add.append(keyLabel, keyInput, valueLabel, valueInput, addButton);
    body.append(add);

    const actions = document.createElement("div");
    actions.className = "env-actions";
    const save = document.createElement("button");
    save.type = "button";
    save.className = "button button-primary env-save";
    save.textContent = "Save";
    save.addEventListener("click", () => {
      const payload: Record<string, string> = {};
      for (const row of scopeRows(scope)) {
        payload[row.key] = row.value;
      }
      save.disabled = true;
      void api
        .putEnv(payload)
        .then(() => {
          toasts.show("Saved to disk", "success");
          // The env shadow raises the dirty count and the Apply button.
          void store.load();
        })
        .catch((error: unknown) => {
          if (!(error instanceof UnauthorizedError)) {
            toasts.show(error instanceof Error ? error.message : "The save failed", "error");
          }
        })
        .finally(() => {
          save.disabled = false;
        });
    });
    actions.append(save);
    body.append(actions);

    const note = document.createElement("p");
    note.className = "field-help env-note";
    note.textContent =
      "Saves a pending shadow; Apply promotes it. The global environment loads only at startup, so changes take effect after a gateway restart.";
    body.append(note);
    return body;
  };

  /** The single global environment section. */
  const globalSection = (): HTMLElement => {
    const section = document.createElement("section");
    section.className = "settings-card env-section";
    section.dataset["scope"] = "global";
    const render = (): void => {
      const heading = document.createElement("h2");
      heading.className = "section-heading";
      const file = env?.global ? ` (${fileName(env.global.path)})` : "";
      heading.textContent = `Global environment${file}`;
      if (env?.global) {
        section.replaceChildren(heading, sectionBody("global", render));
      } else {
        const empty = document.createElement("p");
        empty.className = "view-empty";
        empty.textContent = "No global environment file is configured.";
        section.replaceChildren(heading, empty);
      }
    };
    render();
    return section;
  };

  const render = (): void => {
    if (!main) {
      return;
    }
    const title = document.createElement("h1");
    title.className = "view-title";
    title.textContent = "Secrets";
    main.replaceChildren(title, globalSection());
  };

  return {
    mount(target: HTMLElement): () => void {
      main = target;
      loadController?.abort();
      const controller = new AbortController();
      loadController = controller;
      const title = document.createElement("h1");
      title.className = "view-title";
      title.textContent = "Secrets";
      const loading = document.createElement("p");
      loading.className = "view-empty";
      loading.textContent = "Loading\u2026";
      target.replaceChildren(title, loading);
      void load(controller.signal)
        .then(render)
        .catch((error: unknown) => {
          if (
            error instanceof UnauthorizedError ||
            (error !== null &&
              typeof error === "object" &&
              "name" in error &&
              error.name === "AbortError")
          ) {
            return;
          }
          const failed = document.createElement("p");
          failed.className = "view-empty";
          failed.textContent =
            error instanceof Error ? error.message : "The env files could not be read.";
          target.replaceChildren(title, failed);
        });
      return () => {
        controller.abort();
        probeController?.abort();
        probeController = null;
        if (loadController === controller) {
          loadController = null;
        }
        main = null;
      };
    },
  };
}
