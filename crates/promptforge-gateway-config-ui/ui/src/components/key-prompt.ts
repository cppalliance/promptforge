// The first-load API key screen [Adapted: Unsloth] (standalone only):
// a centered card with the cold medallion, the product title, and a
// labeled password input. A verified key lands in sessionStorage and
// the shell mounts; a rejected key shows the inline error.

import type { GatewayApi } from "../services/gateway-api";

/** Construction dependencies for the key prompt. */
export interface KeyPromptDeps {
  /** The admin API client; `verifyKey` probes and stores the key. */
  api: GatewayApi;
  /** Called once a key verifies; the caller mounts the shell. */
  onSuccess: () => void;
}

/** Replaces `root`'s content with the key prompt screen. */
export function mountKeyPrompt(root: HTMLElement, deps: KeyPromptDeps): void {
  const screen = document.createElement("div");
  screen.className = "key-prompt";

  const card = document.createElement("section");
  card.className = "key-card";

  // Decorative: the title beside it names the product.
  const medallion = document.createElement("img");
  medallion.src = "icons/promptforge-icon-1.png";
  medallion.alt = "";
  medallion.width = 64;
  medallion.height = 64;

  const title = document.createElement("h1");
  title.textContent = "PromptForge Gateway";

  const form = document.createElement("form");

  const label = document.createElement("label");
  label.htmlFor = "gateway-api-key";
  label.textContent = "API key";

  const input = document.createElement("input");
  input.type = "password";
  input.id = "gateway-api-key";
  input.name = "key";
  input.className = "input";
  input.autocomplete = "current-password";
  input.required = true;

  const error = document.createElement("p");
  error.className = "field-error";
  error.id = "gateway-api-key-error";
  error.hidden = true;

  const submit = document.createElement("button");
  submit.type = "submit";
  submit.className = "button button-primary";
  submit.textContent = "Connect";

  form.append(label, input, error, submit);
  card.append(medallion, title, form);
  screen.append(card);
  root.replaceChildren(screen);
  input.focus();

  const showError = (message: string) => {
    error.textContent = message;
    error.hidden = false;
    input.setAttribute("aria-invalid", "true");
    input.setAttribute("aria-describedby", error.id);
  };

  const clearError = () => {
    error.hidden = true;
    input.removeAttribute("aria-invalid");
    input.removeAttribute("aria-describedby");
  };

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    void (async () => {
      clearError();
      submit.disabled = true;
      let valid: boolean;
      try {
        valid = await deps.api.verifyKey(input.value);
      } catch {
        showError("Gateway unreachable");
        submit.disabled = false;
        return;
      }
      if (valid) {
        deps.onSuccess();
        return;
      }
      showError("Invalid API key");
      submit.disabled = false;
    })();
  });
}
