// Composition root for the gateway config SPA. For now it mounts a
// placeholder shell so the esbuild pipeline has real input; the tab bar,
// router, and views land in later commits and replace this body.

/** Mounts the placeholder shell into the given root element. */
export function mountShell(root: HTMLElement): void {
  const shell = document.createElement("div");
  shell.className = "shell";
  const title = document.createElement("h1");
  title.className = "shell-title";
  title.textContent = "PromptForge Gateway Config";
  shell.append(title);
  root.append(shell);
}

const app = document.querySelector<HTMLElement>("#app");
if (app) {
  mountShell(app);
}
