// The inline alert under a bad entry: a 12px/16px message in the IDE danger
// red (the shared `.field-error` skin) right after the input, announced as an
// alert, with `aria-invalid` and `aria-describedby` on the input so assistive
// tech reads the reason with the field.
//
// There is no clear call: a good entry commits an edit, and the commit
// re-renders the page, which discards the input and its alert together.

/** The alert each input currently shows, so a later call updates it in place. */
const alerts = new WeakMap<HTMLElement, HTMLElement>();

/**
 * Shows `message` as an alert directly after `input`, updating the alert in
 * place when the input already has one. The input needs an id: the alert's
 * id is derived from it.
 */
export function setFieldError(input: HTMLElement, message: string): void {
  let alert = alerts.get(input);
  if (alert === undefined) {
    alert = document.createElement("p");
    alert.id = `${input.id}-error`;
    alert.className = "field-error";
    alert.setAttribute("role", "alert");
    alerts.set(input, alert);
    input.after(alert);
    const describedBy = (input.getAttribute("aria-describedby") ?? "").split(" ").filter(Boolean);
    input.setAttribute("aria-describedby", [...describedBy, alert.id].join(" "));
  }
  alert.textContent = message;
  input.setAttribute("aria-invalid", "true");
}
