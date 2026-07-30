pub fn browser_takeover_script(token: &str) -> String {
    let takeover_url = format!("nexa-user-input://{token}");
    let encoded_url = serde_json::to_string(&takeover_url)
        .expect("browser takeover URL must be JSON serializable");
    r#"
(() => {
  const marker = '__NEXA_AUTHENTICATED_TAKEOVER__';
  const takeoverUrl = __NEXA_TAKEOVER_URL__;
  const apply = Reflect.apply;
  const nativePostMessage = Window.prototype.postMessage;
  const nativeStopImmediatePropagation = Event.prototype.stopImmediatePropagation;
  let pending = false;

  const navigateSignal = () => {
    if (pending) return;
    pending = true;
    if (window === window.top) {
      window.location.href = takeoverUrl;
    } else {
      apply(nativePostMessage, window.top, [{ marker, takeoverUrl }, '*']);
    }
    setTimeout(() => { pending = false; }, 100);
  };

  if (window === window.top) {
    addEventListener('message', (event) => {
      if (!event.data || event.data.marker !== marker) return;
      apply(nativeStopImmediatePropagation, event, []);
      if (event.data.takeoverUrl === takeoverUrl) window.location.href = takeoverUrl;
    }, true);
  }

  for (const type of ['pointerdown', 'keydown', 'input']) {
    addEventListener(type, (event) => {
      if (event.isTrusted) navigateSignal();
    }, true);
  }
})();
"#
    .replace("__NEXA_TAKEOVER_URL__", &encoded_url)
}

pub const BROWSER_INIT_SCRIPT: &str = r#"
(() => {
  if (window.__NEXA_BROWSER_RUNTIME__) return;
  const runtime = {
    refs: new Map(),
    observationSeq: 0,
    userEpoch: 0,
    synthetic: false,
    pickMode: null,
    pendingArtifact: null,
    overlay: null,
    regionStart: null,
  };

  const textOf = (el) => String(
    el.getAttribute?.('aria-label') || el.innerText || el.getAttribute?.('placeholder') || el.getAttribute?.('name') || ''
  ).trim().slice(0, 240);
  const roleOf = (el) => el.getAttribute?.('role') || ({
    A: 'link', BUTTON: 'button', INPUT: 'textbox', TEXTAREA: 'textbox', SELECT: 'combobox'
  }[el.tagName] || '');
  const cssPath = (el) => {
    const parts = [];
    let current = el;
    while (current?.nodeType === 1 && parts.length < 6) {
      let part = current.tagName.toLowerCase();
      if (current.id) { part += `#${CSS.escape(current.id)}`; parts.unshift(part); break; }
      const parent = current.parentElement;
      if (parent) {
        const siblings = Array.from(parent.children).filter((item) => item.tagName === current.tagName);
        if (siblings.length > 1) part += `:nth-of-type(${siblings.indexOf(current) + 1})`;
      }
      parts.unshift(part);
      current = parent;
    }
    return parts.join(' > ');
  };
  const roots = () => {
    const result = [document];
    const visit = (root) => {
      for (const element of root.querySelectorAll?.('*') || []) {
        if (element.shadowRoot) { result.push(element.shadowRoot); visit(element.shadowRoot); }
        if (element.tagName === 'IFRAME') {
          try { if (element.contentDocument) { result.push(element.contentDocument); visit(element.contentDocument); } } catch (_) {}
        }
      }
    };
    visit(document);
    return result;
  };
  const isObservable = (element) => {
    if (String(element.tagName || '').toUpperCase() === 'INPUT' && String(element.type || '').toLowerCase() === 'hidden') return false;
    const rect = element.getBoundingClientRect();
    const style = getComputedStyle(element);
    return rect.width > 0 && rect.height > 0 && style.display !== 'none' && style.visibility !== 'hidden';
  };
  const interactiveElements = () => {
    const selector = 'a[href],button,input:not([type="hidden" i]),textarea,select,[contenteditable="true"],[role="button"],[role="link"],[role="textbox"],[tabindex]';
    const seen = new Set();
    const elements = [];
    for (const root of roots()) {
      for (const element of root.querySelectorAll?.(selector) || []) {
        if (!seen.has(element) && isObservable(element)) { seen.add(element); elements.push(element); }
        if (elements.length >= 300) return elements;
      }
    }
    return elements;
  };
  const navigationTargetOf = (el) => {
    if (el.href) return el.href;
    const tag = String(el.tagName || '').toUpperCase();
    const type = String(el.type || (tag === 'BUTTON' ? 'submit' : '')).toLowerCase();
    const submitter = (tag === 'BUTTON' && type === 'submit') || (tag === 'INPUT' && (type === 'submit' || type === 'image'));
    const implicitSubmitInput = tag === 'INPUT' && !['button', 'reset', 'file', 'checkbox', 'radio'].includes(type);
    if ((!submitter && !implicitSubmitInput) || !el.form) return null;
    try {
      return new URL(el.getAttribute?.('formaction') || el.form.getAttribute?.('action') || location.href, document.baseURI).href;
    } catch (_) { return null; }
  };
  const describe = (el, ref) => {
    const rect = el.getBoundingClientRect();
    const name = textOf(el);
    const navigationTarget = navigationTargetOf(el);
    return {
      ref,
      tag: String(el.tagName || '').toLowerCase(),
      role: roleOf(el),
      name,
      href: navigationTarget,
      inputType: el.type || null,
      enabled: !el.disabled,
      visible: rect.width > 0 && rect.height > 0 && getComputedStyle(el).visibility !== 'hidden',
      bounds: { x: rect.x, y: rect.y, width: rect.width, height: rect.height },
      locatorFingerprint: {
        tag: String(el.tagName || '').toLowerCase(),
        id: el.id || null,
        testId: el.getAttribute?.('data-testid') || null,
        name: el.getAttribute?.('name') || null,
        href: navigationTarget,
        cssPath: cssPath(el),
        textHash: name,
      },
    };
  };
  runtime.observe = () => {
    runtime.observationSeq += 1;
    runtime.refs = new Map();
    const elements = interactiveElements().map((el, index) => {
      const ref = `e_${runtime.observationSeq}_${index + 1}`;
      runtime.refs.set(ref, el);
      return describe(el, ref);
    });
    return {
      url: location.href,
      title: document.title,
      text: document.body ? document.body.innerText.slice(0, 30000) : '',
      viewport: { width: innerWidth, height: innerHeight, deviceScaleFactor: devicePixelRatio },
      historyLength: history.length,
      userEpoch: runtime.userEpoch,
      domFingerprint: `${location.href}|${document.documentElement?.outerHTML.length || 0}|${document.body?.innerText.slice(0, 20000) || ''}`,
      elements,
    };
  };
  runtime.invalidateForUserTakeover = () => {
    runtime.userEpoch += 1;
    runtime.refs = new Map();
  };
  runtime.act = (input) => {
    if (input.userEpoch !== runtime.userEpoch) throw new Error('stale observation: user interacted with the page');
    const domFingerprint = `${location.href}|${document.documentElement?.outerHTML.length || 0}|${document.body?.innerText.slice(0, 20000) || ''}`;
    if (input.domFingerprint !== domFingerprint) throw new Error('stale observation: page content changed');
    const el = input.targetRef ? runtime.refs.get(input.targetRef) : null;
    if (input.targetRef && (!el || !el.isConnected)) throw new Error('stale observation: target disappeared');
    if (el && input.expected) {
      const current = describe(el, input.targetRef);
      if (current.role !== input.expected.role || current.name !== input.expected.name) {
        throw new Error('stale observation: target identity changed');
      }
      const before = input.expected.bounds;
      const after = current.bounds;
      if (['x','y','width','height'].some((key) => Math.abs(Number(before[key]) - Number(after[key])) > 4)) {
        throw new Error('stale observation: target bounds changed');
      }
    }
    runtime.synthetic = true;
    try {
      if (input.action === 'click') el.click();
      else if (input.action === 'type') {
        el.focus();
        const realm = el.ownerDocument?.defaultView || window;
        if (el.isContentEditable) {
          el.textContent = input.text || '';
        } else {
          const prototype = el instanceof realm.HTMLTextAreaElement ? realm.HTMLTextAreaElement.prototype : realm.HTMLInputElement.prototype;
          const setter = Object.getOwnPropertyDescriptor(prototype, 'value')?.set;
          if (setter) setter.call(el, input.text || ''); else el.value = input.text || '';
        }
        el.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: input.text || '' }));
        el.dispatchEvent(new Event('change', { bubbles: true }));
      } else if (input.action === 'select') {
        el.value = input.value || '';
        el.dispatchEvent(new Event('input', { bubbles: true }));
        el.dispatchEvent(new Event('change', { bubbles: true }));
      } else if (input.action === 'press') {
        const target = el || document.activeElement || document.body;
        const realm = target.ownerDocument?.defaultView || window;
        const key = String(input.key || '');
        if (key === 'Enter') {
          if (target instanceof realm.HTMLAnchorElement || target instanceof realm.HTMLButtonElement || ['submit', 'image'].includes(String(target.type || '').toLowerCase())) {
            target.click();
          } else if (target instanceof realm.HTMLInputElement && target.form) {
            target.form.requestSubmit();
          } else {
            throw new Error('Unsupported browser key for this element: Enter');
          }
        } else if (key === 'Tab') {
          const focusable = interactiveElements();
          const index = Math.max(-1, focusable.indexOf(target));
          focusable[(index + 1) % focusable.length]?.focus();
        } else if (key === ' ' || key === 'Space' || key === 'Spacebar') {
          const tag = String(target.tagName || '').toUpperCase();
          const type = String(target.type || '').toLowerCase();
          if (['A', 'BUTTON'].includes(tag) || ['checkbox', 'radio'].includes(type)) target.click();
          else throw new Error('Unsupported browser key for this element: Space');
        } else if (key === 'Escape' || key === 'Esc') {
          target.blur?.();
        } else if ((key === 'ArrowDown' || key === 'ArrowUp') && target instanceof realm.HTMLSelectElement) {
          const delta = key === 'ArrowDown' ? 1 : -1;
          target.selectedIndex = Math.max(0, Math.min(target.options.length - 1, target.selectedIndex + delta));
          target.dispatchEvent(new Event('input', { bubbles: true }));
          target.dispatchEvent(new Event('change', { bubbles: true }));
        } else if (['Home', 'End', 'PageUp', 'PageDown'].includes(key)) {
          const amount = key === 'Home' ? -document.documentElement.scrollHeight
            : key === 'End' ? document.documentElement.scrollHeight
            : key === 'PageUp' ? -innerHeight : innerHeight;
          window.scrollBy(0, amount);
        } else {
          throw new Error(`Unsupported browser key: ${key}`);
        }
      } else if (input.action === 'scroll') {
        window.scrollBy(Number(input.scrollX || 0), Number(input.scrollY || 0));
      }
    } finally { runtime.synthetic = false; }
    return true;
  };
  const clearOverlay = () => {
    runtime.overlay?.remove();
    runtime.overlay = null;
  };
  const showOverlay = (rect, label) => {
    clearOverlay();
    const overlay = document.createElement('div');
    overlay.style.cssText = `position:fixed;z-index:2147483647;pointer-events:none;left:${rect.x}px;top:${rect.y}px;width:${rect.width}px;height:${rect.height}px;border:2px solid #22d3ee;background:rgba(34,211,238,.12);box-shadow:0 0 0 1px rgba(8,47,73,.7)`;
    if (label) overlay.setAttribute('data-nexa-label', label);
    document.documentElement.appendChild(overlay);
    runtime.overlay = overlay;
  };
  runtime.beginPick = (mode) => { runtime.pickMode = mode; runtime.pendingArtifact = null; runtime.regionStart = null; clearOverlay(); };
  runtime.cancelPick = () => { runtime.pickMode = null; runtime.regionStart = null; clearOverlay(); };
  runtime.takeArtifact = () => { const artifact = runtime.pendingArtifact; runtime.pendingArtifact = null; return artifact; };
  runtime.selectedText = () => String(window.getSelection?.() || '').trim().slice(0, 10000);

  addEventListener('pointermove', (event) => {
    if (runtime.pickMode !== 'element') return;
    const target = event.composedPath?.().find((item) => item instanceof Element);
    if (target) showOverlay(target.getBoundingClientRect(), `${roleOf(target)} ${textOf(target)}`);
  }, true);
  addEventListener('pointerdown', (event) => {
    if (!runtime.synthetic && event.isTrusted) {
      runtime.userEpoch += 1;
    }
    if (runtime.pickMode === 'region') {
      runtime.regionStart = { x: event.clientX, y: event.clientY };
      event.preventDefault(); event.stopImmediatePropagation();
    }
  }, true);
  addEventListener('pointermove', (event) => {
    if (runtime.pickMode !== 'region' || !runtime.regionStart) return;
    const x = Math.min(runtime.regionStart.x, event.clientX);
    const y = Math.min(runtime.regionStart.y, event.clientY);
    showOverlay({ x, y, width: Math.abs(event.clientX - runtime.regionStart.x), height: Math.abs(event.clientY - runtime.regionStart.y) }, 'Selected region');
    event.preventDefault(); event.stopImmediatePropagation();
  }, true);
  addEventListener('pointerup', (event) => {
    if (runtime.pickMode !== 'region' || !runtime.regionStart) return;
    const bounds = { x: Math.min(runtime.regionStart.x, event.clientX), y: Math.min(runtime.regionStart.y, event.clientY), width: Math.abs(event.clientX - runtime.regionStart.x), height: Math.abs(event.clientY - runtime.regionStart.y) };
    runtime.pendingArtifact = { kind: 'region', capture: 'coordinatesOnly', url: location.href, title: document.title, bounds, userEpoch: runtime.userEpoch };
    runtime.cancelPick();
    event.preventDefault(); event.stopImmediatePropagation();
  }, true);
  addEventListener('click', (event) => {
    if (runtime.pickMode !== 'element') return;
    const target = event.composedPath?.().find((item) => item instanceof Element);
    if (target) {
      const ref = `picked_${Date.now()}`;
      runtime.refs.set(ref, target);
      runtime.pendingArtifact = { kind: 'element', url: location.href, title: document.title, userEpoch: runtime.userEpoch, ...describe(target, ref) };
    }
    runtime.cancelPick();
    event.preventDefault(); event.stopImmediatePropagation();
  }, true);
  addEventListener('keydown', (event) => {
    if (!runtime.synthetic && event.isTrusted) {
      runtime.userEpoch += 1;
    }
    if (event.key === 'Escape' && runtime.pickMode) { runtime.cancelPick(); event.preventDefault(); event.stopImmediatePropagation(); }
  }, true);
  addEventListener('input', (event) => {
    if (!runtime.synthetic && event.isTrusted) {
      runtime.userEpoch += 1;
    }
  }, true);
  const bridge = Object.freeze({
    observe: () => runtime.observe(),
    act: (input) => runtime.act(input),
    beginPick: (mode) => runtime.beginPick(mode),
    cancelPick: () => runtime.cancelPick(),
    takeArtifact: () => runtime.takeArtifact(),
    selectedText: () => runtime.selectedText(),
  });
  Object.defineProperty(window, '__NEXA_BROWSER_RUNTIME__', {
    value: bridge,
    configurable: false,
    enumerable: false,
    writable: false,
  });
})();
"#;

pub const OBSERVE_EXPRESSION: &str = "window.__NEXA_BROWSER_RUNTIME__?.observe()";
