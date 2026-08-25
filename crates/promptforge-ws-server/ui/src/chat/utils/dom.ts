/**
 * Finds an element inside the container and throws a clear error if it is not present.
 * This ensures the Fail-Fast principle.
 */
export function queryOrThrow<T extends HTMLElement>(context: HTMLElement, selector: string): T {
	const el = context.querySelector(selector);
	if (!el) {
		throw new Error(`DOM Error: Required element "${selector}" not found inside the container.`);
	}
	return el as T;
}

export function el<K extends keyof HTMLElementTagNameMap>(
	tag: K,
	className?: string,
	props?: Partial<HTMLElementTagNameMap[K]> | null,
	children?: (HTMLElement | string | null | false | undefined)[],
): HTMLElementTagNameMap[K] {
	const element = document.createElement(tag);

	if (className) {
		element.className = className;
	}

	if (props) {
		Object.assign(element, props);
	}

	if (children) {
		for (const child of children) {
			if (child) element.append(child);
		}
	}

	return element;
}

export function replaceNodes(parent: HTMLElement, ...nodes: (Node | string)[]): void {
	if (typeof parent.replaceChildren === "function") {
		parent.replaceChildren(...nodes);
		return;
	}

	parent.textContent = "";
	for (const node of nodes) {
		parent.appendChild(typeof node === "string" ? document.createTextNode(node) : node);
	}
}

/**
 * Super lightweight child-node diffing specifically for our sanitized HTML.
 * Mutates `target` children to match `source` children without destroying untouched nodes.
 */
export function syncDOMChildren(target: Node, source: Node) {
	let targetChild = target.firstChild;
	let sourceChild = source.firstChild;

	while (sourceChild !== null) {
		if (targetChild === null) {
			// Target is missing children; append the remainder
			target.appendChild(sourceChild.cloneNode(true));
			sourceChild = sourceChild.nextSibling;
		} else {
			// Cache next siblings before recursion in case targetChild replaces itself
			const nextTargetChild = targetChild.nextSibling;
			const nextSourceChild = sourceChild.nextSibling;

			syncDOMNode(targetChild, sourceChild);

			targetChild = nextTargetChild;
			sourceChild = nextSourceChild;
		}
	}

	// Cleanup remaining obsolete target children
	while (targetChild !== null) {
		const nextTargetChild = targetChild.nextSibling;
		target.removeChild(targetChild);
		targetChild = nextTargetChild;
	}
}

function syncDOMNode(target: Node, source: Node) {
	// Reconcile text nodes
	if (target.nodeType === Node.TEXT_NODE && source.nodeType === Node.TEXT_NODE) {
		if (target.nodeValue !== source.nodeValue) {
			target.nodeValue = source.nodeValue;
		}
		return;
	}

	// Replace entirely if node types or tags diverge
	if (target.nodeType !== source.nodeType || target.nodeName !== source.nodeName) {
		target.parentNode?.replaceChild(source.cloneNode(true), target);
		return;
	}

	// Reconcile attributes (Elements only)
	if (target.nodeType === Node.ELEMENT_NODE) {
		const elTarget = target as HTMLElement;
		const elSource = source as HTMLElement;

		const sourceAttrs = elSource.attributes;
		const targetAttrs = elTarget.attributes;

		// Remove obsolete attributes.
		// Note: targetAttrs is a live NamedNodeMap, so backward iteration is required.
		for (let i = targetAttrs.length - 1; i >= 0; i--) {
			const attrName = targetAttrs[i].name;
			if (!elSource.hasAttribute(attrName)) {
				elTarget.removeAttribute(attrName);
			}
		}

		// Add or update existing attributes
		for (let i = 0; i < sourceAttrs.length; i++) {
			const attr = sourceAttrs[i];
			if (elTarget.getAttribute(attr.name) !== attr.value) {
				elTarget.setAttribute(attr.name, attr.value);
			}
		}
	}

	syncDOMChildren(target, source);
}
