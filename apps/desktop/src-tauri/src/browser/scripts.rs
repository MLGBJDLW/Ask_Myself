pub fn browser_takeover_script(token: &str) -> String {
    let takeover_url = format!("nexa-user-input://{token}");
    let encoded_url = serde_json::to_string(&takeover_url)
        .expect("browser takeover URL must be JSON serializable");
    let encoded_token =
        serde_json::to_string(token).expect("browser takeover token must be JSON serializable");
    r#"
(() => {
  const marker = '__NEXA_AUTHENTICATED_TAKEOVER__';
  const takeoverUrl = __NEXA_TAKEOVER_URL__;
  const trustedInputMarker = '__NEXA_AUTHENTICATED_TRUSTED_INPUT__';
  const trustedInputToken = __NEXA_TAKEOVER_TOKEN__;
  const apply = Reflect.apply;
  const nativePostMessage = Window.prototype.postMessage;
  const nativeStopImmediatePropagation = Event.prototype.stopImmediatePropagation;
  let pending = false;
  let trustedInputGuard = null;

  const normalizedTrustedBudget = (budget) => {
    if (!budget || typeof budget !== 'object') return null;
    const pointerDown = Number(budget.pointerDown);
    const keyDown = Number(budget.keyDown);
    const input = Number(budget.input);
    if (!Number.isInteger(pointerDown) || pointerDown < 0 || pointerDown > 2) return null;
    if (!Number.isInteger(keyDown) || keyDown < 0 || keyDown > 1) return null;
    if (!Number.isInteger(input) || input < 0 || input > 2) return null;
    if (pointerDown + keyDown + input === 0) return null;
    return { pointerDown, keyDown, input };
  };

  const normalizedTrustedExpectation = (expected) => {
    if (!expected || typeof expected !== 'object') return null;
    if (expected.kind === 'pointer') {
      const x = Number(expected.x);
      const y = Number(expected.y);
      const button = String(expected.button || '');
      if (!Number.isFinite(x) || !Number.isFinite(y) || x < 0 || y < 0) return null;
      if (!['left', 'middle', 'right'].includes(button)) return null;
      return { kind: 'pointer', x, y, button };
    }
    if (expected.kind === 'key') {
      const key = String(expected.key || '');
      return key && key.length <= 32 ? { kind: 'key', key } : null;
    }
    if (expected.kind === 'text') {
      const data = String(expected.data ?? '');
      return data.length <= 262144 ? { kind: 'text', data } : null;
    }
    return null;
  };

  const armTrustedInputLocal = (operationId, budget, expected) => {
    const normalized = normalizedTrustedBudget(budget);
    const normalizedExpected = normalizedTrustedExpectation(expected);
    if (!normalized || !normalizedExpected || typeof operationId !== 'string' || !operationId || trustedInputGuard) return false;
    const target = normalizedExpected.kind === 'pointer'
      ? document.elementFromPoint(normalizedExpected.x, normalizedExpected.y)
      : document.activeElement;
    if (!target || target === document.body || target === document.documentElement) return false;
    trustedInputGuard = {
      operationId,
      expected: normalizedExpected,
      target,
      ...normalized,
    };
    return true;
  };

  const disarmTrustedInputLocal = (operationId) => {
    if (trustedInputGuard && trustedInputGuard.operationId !== operationId) return false;
    trustedInputGuard = null;
    return true;
  };

  const expectationForFrame = (expected, frame) => {
    if (!expected || expected.kind !== 'pointer') return expected;
    const rect = frame.getBoundingClientRect();
    return { ...expected, x: expected.x - rect.left, y: expected.y - rect.top };
  };

  const relayTrustedInput = (kind, operationId, budget = null, expected = null) => {
    for (const frame of Array.from(document.querySelectorAll('iframe'))) {
      try {
        if (frame.contentWindow) {
          apply(nativePostMessage, frame.contentWindow, [{
            marker: trustedInputMarker,
            token: trustedInputToken,
            kind,
            operationId,
            budget,
            expected: expectationForFrame(expected, frame),
          }, '*']);
        }
      } catch (_) {}
    }
  };

  const trustedInputApi = Object.freeze({
    expects: (type, event) => matchesTrustedInput(type, event),
    arm: (providedToken, operationId, budget, expected) => {
      if (providedToken !== trustedInputToken) return false;
      const armed = armTrustedInputLocal(operationId, budget, expected);
      if (armed) relayTrustedInput('arm', operationId, budget, expected);
      return armed;
    },
    disarm: (providedToken, operationId) => {
      if (providedToken !== trustedInputToken) return false;
      const disarmed = disarmTrustedInputLocal(operationId);
      if (disarmed) relayTrustedInput('disarm', operationId);
      return disarmed;
    },
  });
  Object.defineProperty(window, '__NEXA_TRUSTED_INPUT_GUARD__', {
    value: trustedInputApi,
    configurable: false,
    enumerable: false,
    writable: false,
  });

  addEventListener('message', (event) => {
    const message = event.data;
    if (window === window.top || event.source !== window.parent) return;
    if (!message || message.marker !== trustedInputMarker || message.token !== trustedInputToken) return;
    apply(nativeStopImmediatePropagation, event, []);
    if (message.kind === 'arm') {
      if (armTrustedInputLocal(message.operationId, message.budget, message.expected)) {
        relayTrustedInput('arm', message.operationId, message.budget, message.expected);
      }
    } else if (message.kind === 'disarm') {
      if (disarmTrustedInputLocal(message.operationId)) {
        relayTrustedInput('disarm', message.operationId);
      }
    }
  }, true);

  const eventTargetsArmedElement = (event) => {
    const target = trustedInputGuard?.target;
    if (!target || target === document.body || target === document.documentElement) return true;
    return event.target === target || Boolean(target.contains?.(event.target));
  };

  const matchesTrustedInput = (type, event) => {
    if (!trustedInputGuard) return false;
    const budgetKey = ({ pointerdown: 'pointerDown', keydown: 'keyDown', input: 'input' })[type];
    if (!budgetKey || trustedInputGuard[budgetKey] <= 0) return false;
    const expected = trustedInputGuard.expected;
    if (type === 'pointerdown') {
      const button = ({ 0: 'left', 1: 'middle', 2: 'right' })[event.button];
      return expected.kind === 'pointer'
        && button === expected.button
        && Math.abs(event.clientX - expected.x) <= 2
        && Math.abs(event.clientY - expected.y) <= 2
        && eventTargetsArmedElement(event);
    }
    if (type === 'keydown') {
      return expected.kind === 'key' && event.key === expected.key && eventTargetsArmedElement(event);
    }
    if (expected.kind === 'text') {
      return event.data === expected.data && eventTargetsArmedElement(event);
    }
    if (expected.kind === 'key') return eventTargetsArmedElement(event);
    if (expected.kind === 'pointer') {
      const rect = event.target?.getBoundingClientRect?.();
      return eventTargetsArmedElement(event) && Boolean(rect
        && expected.x >= rect.left && expected.x <= rect.right
        && expected.y >= rect.top && expected.y <= rect.bottom);
    }
    return false;
  };

  const consumeTrustedInput = (type, event) => {
    if (!matchesTrustedInput(type, event)) {
      trustedInputGuard = null;
      return false;
    }
    const budgetKey = ({ pointerdown: 'pointerDown', keydown: 'keyDown', input: 'input' })[type];
    trustedInputGuard[budgetKey] -= 1;
    if (trustedInputGuard.pointerDown === 0 && trustedInputGuard.keyDown === 0 && trustedInputGuard.input === 0) {
      trustedInputGuard = null;
    }
    return true;
  };

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

  for (const type of ['pointerdown', 'keydown', 'input', 'wheel', 'touchstart']) {
    addEventListener(type, (event) => {
      if (event.isTrusted && !consumeTrustedInput(type, event)) navigateSignal();
    }, true);
  }
})();
"#
    .replace("__NEXA_TAKEOVER_URL__", &encoded_url)
    .replace("__NEXA_TAKEOVER_TOKEN__", &encoded_token)
}

pub fn browser_init_script(pick_token: &str) -> String {
    let encoded_token =
        serde_json::to_string(pick_token).expect("browser picker token must be JSON serializable");
    BROWSER_INIT_SCRIPT.replace("__NEXA_PICK_TOKEN__", &encoded_token)
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
    agentCursor: null,
    cursorDocument: null,
    cursorPoint: null,
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
  const dragDestinationElements = () => {
    const selector = '[draggable="true"],[ondrop],[ondragover],[data-dropzone],[data-drop-zone],[class*="drop" i],[id*="drop" i]';
    const elements = [];
    for (const root of roots()) {
      for (const element of root.querySelectorAll?.(selector) || []) {
        if (isObservable(element)) elements.push(element);
        if (elements.length >= 100) return elements;
      }
    }
    return elements;
  };
  const observableElements = () => {
    const seen = new Set();
    return [...interactiveElements(), ...dragDestinationElements()].filter((element) => {
      if (seen.has(element)) return false;
      seen.add(element);
      return true;
    }).slice(0, 400);
  };
  const navigationTargetOf = (el) => {
    if (el.href) return el.href;
    const tag = String(el.tagName || '').toUpperCase();
    const type = String(el.type || (tag === 'BUTTON' ? 'submit' : '')).toLowerCase();
    const submitter = (tag === 'BUTTON' && type === 'submit') || (tag === 'INPUT' && (type === 'submit' || type === 'image'));
    const implicitSubmitInput = tag === 'INPUT' && !['button', 'reset', 'file', 'checkbox', 'radio'].includes(type);
    if ((!submitter && !implicitSubmitInput) || !el.form) return null;
    try {
      const ownerDocument = el.ownerDocument || document;
      const fallbackUrl = ownerDocument.location?.href || location.href;
      return new URL(el.getAttribute?.('formaction') || el.form.getAttribute?.('action') || fallbackUrl, ownerDocument.baseURI).href;
    } catch (_) { return null; }
  };
  const viewportBoundsOf = (el) => {
    const rect = el.getBoundingClientRect();
    let x = rect.x;
    let y = rect.y;
    let ownerWindow = el.ownerDocument?.defaultView;
    while (ownerWindow && ownerWindow !== window.top) {
      try {
        const frame = ownerWindow.frameElement;
        if (!frame) break;
        const frameBounds = frame.getBoundingClientRect();
        x += frameBounds.x;
        y += frameBounds.y;
        ownerWindow = frame.ownerDocument?.defaultView;
      } catch (_) { break; }
    }
    return { x, y, width: rect.width, height: rect.height };
  };
  const describe = (el, ref) => {
    const rect = viewportBoundsOf(el);
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
  const hashText = (value) => {
    let hash = 2166136261;
    for (let index = 0; index < value.length; index += 1) {
      hash ^= value.charCodeAt(index);
      hash = Math.imul(hash, 16777619);
    }
    return (hash >>> 0).toString(16).padStart(8, '0');
  };
  const domFingerprintOf = () => {
    const visualLength = Array.from(document.querySelectorAll('[data-nexa-agent-cursor],[data-nexa-agent-click]'))
      .reduce((total, element) => total + element.outerHTML.length, 0);
    const markupLength = Math.max(0, (document.documentElement?.outerHTML.length || 0) - visualLength);
    const interactiveState = observableElements().map((element) => {
      const rect = viewportBoundsOf(element);
      const style = getComputedStyle(element);
      return [
        String(element.tagName || '').toLowerCase(),
        element.id || '',
        element.getAttribute?.('name') || '',
        element.getAttribute?.('role') || '',
        element.getAttribute?.('aria-label') || '',
        navigationTargetOf(element) || '',
        element.type || '',
        element.disabled ? 'disabled' : 'enabled',
        element.checked ? 'checked' : '',
        Number.isInteger(element.selectedIndex) ? String(element.selectedIndex) : '',
        style.display,
        style.visibility,
        style.opacity,
        Math.round(rect.x),
        Math.round(rect.y),
        Math.round(rect.width),
        Math.round(rect.height),
        textOf(element),
      ].join('\u001f');
    }).join('\u001e');
    const bodyText = document.body?.innerText.slice(0, 30000) || '';
    return `v2|${location.href}|${scrollX}|${scrollY}|${markupLength}|${hashText(bodyText)}|${hashText(interactiveState)}`;
  };
  const actionVerificationBaseline = () => ({
    url: location.href,
    userEpoch: runtime.userEpoch,
    domFingerprint: domFingerprintOf(),
  });
  runtime.observe = () => {
    runtime.observationSeq += 1;
    runtime.refs = new Map();
    const elements = observableElements().map((el, index) => {
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
      domFingerprint: domFingerprintOf(),
      elements,
    };
  };
  const validateAction = (input) => {
    if (input.userEpoch !== runtime.userEpoch) throw new Error('stale observation: user interacted with the page');
    const domFingerprint = domFingerprintOf();
    if (input.domFingerprint !== domFingerprint) throw new Error('stale observation: page content changed');
    const el = input.targetRef ? runtime.refs.get(input.targetRef) : null;
    const end = input.endRef ? runtime.refs.get(input.endRef) : null;
    if (input.targetRef && (!el || !el.isConnected)) throw new Error('stale observation: target disappeared');
    if (input.endRef && (!end || !end.isConnected)) throw new Error('stale observation: drag destination disappeared');
    const verify = (element, ref, expected, label) => {
      if (!element || !expected) return;
      const current = describe(element, ref);
      if (current.role !== expected.role || current.name !== expected.name) {
        throw new Error('stale observation: target identity changed');
      }
      const before = expected.bounds;
      const after = current.bounds;
      if (['x','y','width','height'].some((key) => Math.abs(Number(before[key]) - Number(after[key])) > 4)) {
        throw new Error(`stale observation: ${label} bounds changed`);
      }
    };
    verify(el, input.targetRef, input.expected, 'target');
    verify(end, input.endRef, input.expectedEnd, 'drag destination');
    return { el, end };
  };
  const centerOf = (el) => {
    const rect = el.getBoundingClientRect();
    return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
  };
  const ensureAgentCursor = (ownerDocument) => {
    if (runtime.agentCursor?.isConnected && runtime.cursorDocument === ownerDocument) return runtime.agentCursor;
    runtime.agentCursor?.remove();
    const cursor = ownerDocument.createElement('div');
    cursor.setAttribute('data-nexa-agent-cursor', 'true');
    cursor.setAttribute('aria-hidden', 'true');
    cursor.style.cssText = 'position:fixed;left:0;top:0;width:22px;height:28px;z-index:2147483647;pointer-events:none;will-change:transform;filter:drop-shadow(0 2px 4px rgba(2,6,23,.45));contain:layout paint style;';
    cursor.innerHTML = "<svg viewBox='0 0 22 28' width='22' height='28' xmlns='http://www.w3.org/2000/svg'><path d='M2 1.75v20.5l5.4-5.2 3.45 8.15 4.2-1.8-3.5-8.05h7.35L2 1.75Z' fill='#f8fafc' stroke='#0891b2' stroke-width='1.8' stroke-linejoin='round'/></svg>";
    (ownerDocument.documentElement || ownerDocument.body).appendChild(cursor);
    runtime.agentCursor = cursor;
    runtime.cursorDocument = ownerDocument;
    runtime.cursorPoint = null;
    return cursor;
  };
  const moveAgentCursor = (el, via = null) => {
    if (!el) return 0;
    const ownerDocument = el.ownerDocument || document;
    const ownerWindow = ownerDocument.defaultView || window;
    const cursor = ensureAgentCursor(ownerDocument);
    const to = centerOf(el);
    const viaPoint = via ? centerOf(via) : null;
    const from = runtime.cursorPoint || {
      x: Math.max(18, Number(ownerWindow.innerWidth || 0) * 0.22),
      y: Math.max(22, Number(ownerWindow.innerHeight || 0) * 0.18),
    };
    const distance = viaPoint
      ? Math.hypot(viaPoint.x - from.x, viaPoint.y - from.y) + Math.hypot(to.x - viaPoint.x, to.y - viaPoint.y)
      : Math.hypot(to.x - from.x, to.y - from.y);
    const reduced = ownerWindow.matchMedia?.('(prefers-reduced-motion: reduce)').matches;
    const duration = reduced ? 0 : Math.round(Math.min(520, Math.max(180, 150 + distance * 0.36)));
    const translate = (point) => `translate3d(${point.x}px,${point.y}px,0)`;
    cursor.getAnimations?.().forEach((animation) => animation.cancel());
    if (duration > 0 && cursor.animate) {
      const bend = Math.min(70, Math.max(18, distance * 0.14));
      const frames = viaPoint ? [
        { transform: translate(from), offset: 0 },
        { transform: translate(viaPoint), offset: 0.48 },
        { transform: translate(to), offset: 1 },
      ] : [
        { transform: translate(from) },
        { transform: translate({ x: from.x + (to.x - from.x) * 0.55, y: from.y + (to.y - from.y) * 0.45 - bend }) },
        { transform: translate(to) },
      ];
      cursor.animate(frames, { duration, easing: 'cubic-bezier(.22,.8,.24,1)', fill: 'forwards' });
    }
    cursor.style.transform = translate(to);
    runtime.cursorPoint = to;
    return duration;
  };
  const mouseButtonOf = (input) => ({ left: 0, middle: 1, right: 2 })[input.button] ?? 0;
  const pressedButtonsOf = (input) => ({ left: 1, middle: 4, right: 2 })[input.button] ?? 1;
  const pointerOptions = (input, point, detail = 1, pressed = false) => ({
    bubbles: true,
    cancelable: true,
    composed: true,
    clientX: point.x,
    clientY: point.y,
    button: mouseButtonOf(input),
    buttons: pressed ? pressedButtonsOf(input) : 0,
    pointerId: 1,
    pointerType: 'mouse',
    isPrimary: true,
    detail,
    altKey: input.modifiers?.includes('Alt') || false,
    ctrlKey: input.modifiers?.includes('Control') || false,
    metaKey: input.modifiers?.includes('Meta') || false,
    shiftKey: input.modifiers?.includes('Shift') || false,
  });
  const dispatchPointer = (el, type, input, point, detail = 1, pressed = false) => {
    const realm = el.ownerDocument?.defaultView || window;
    const EventType = type.startsWith('pointer') && realm.PointerEvent ? realm.PointerEvent : realm.MouseEvent;
    el.dispatchEvent(new EventType(type, pointerOptions(input, point, detail, pressed)));
  };
  const pulseAt = (el, point) => {
    const ownerDocument = el.ownerDocument || document;
    const pulse = ownerDocument.createElement('div');
    pulse.setAttribute('data-nexa-agent-click', 'true');
    pulse.style.cssText = `position:fixed;z-index:2147483646;pointer-events:none;left:${point.x - 11}px;top:${point.y - 11}px;width:22px;height:22px;border:2px solid #22d3ee;border-radius:999px;box-sizing:border-box;`;
    (ownerDocument.documentElement || ownerDocument.body).appendChild(pulse);
    const animation = pulse.animate?.([
      { opacity: 1, transform: 'scale(.35)' },
      { opacity: 0, transform: 'scale(1.65)' },
    ], { duration: 360, easing: 'ease-out' });
    if (animation) animation.onfinish = () => pulse.remove(); else setTimeout(() => pulse.remove(), 360);
  };
  const hoverAt = (el, input, point) => {
    for (const type of ['pointerover', 'mouseover', 'pointermove', 'mousemove']) dispatchPointer(el, type, input, point);
  };
  const clickAt = (el, input, detail = 1) => {
    const point = centerOf(el);
    hoverAt(el, input, point);
    dispatchPointer(el, 'pointerdown', input, point, detail, true);
    dispatchPointer(el, 'mousedown', input, point, detail, true);
    dispatchPointer(el, 'pointerup', input, point, detail);
    dispatchPointer(el, 'mouseup', input, point, detail);
    const button = mouseButtonOf(input);
    if (button === 0 && !(input.modifiers?.length)) el.click();
    else dispatchPointer(el, button === 1 ? 'auxclick' : button === 2 ? 'contextmenu' : 'click', input, point, detail);
    pulseAt(el, point);
  };
  const dragBetween = (source, destination, input) => {
    const realm = source.ownerDocument?.defaultView || window;
    const from = centerOf(source);
    const to = centerOf(destination);
    let transfer = null;
    try { transfer = realm.DataTransfer ? new realm.DataTransfer() : null; } catch (_) {}
    const dragEvent = (target, type, point) => {
      if (realm.DragEvent) target.dispatchEvent(new realm.DragEvent(type, { ...pointerOptions(input, point, 1, true), dataTransfer: transfer }));
      else dispatchPointer(target, type, input, point, 1, true);
    };
    hoverAt(source, input, from);
    dispatchPointer(source, 'pointerdown', input, from, 1, true);
    dispatchPointer(source, 'mousedown', input, from, 1, true);
    dragEvent(source, 'dragstart', from);
    const ownerDocument = source.ownerDocument || document;
    for (let step = 1; step <= 8; step += 1) {
      const progress = step / 8;
      const point = { x: from.x + (to.x - from.x) * progress, y: from.y + (to.y - from.y) * progress };
      const target = ownerDocument === destination.ownerDocument
        ? ownerDocument.elementFromPoint(point.x, point.y) || destination
        : destination;
      dispatchPointer(target, 'pointermove', input, point, 1, true);
      dispatchPointer(target, 'mousemove', input, point, 1, true);
    }
    dragEvent(destination, 'dragenter', to);
    dragEvent(destination, 'dragover', to);
    dragEvent(destination, 'drop', to);
    dragEvent(source, 'dragend', to);
    dispatchPointer(destination, 'pointerup', input, to);
    dispatchPointer(destination, 'mouseup', input, to);
    pulseAt(destination, to);
  };
  runtime.invalidateForUserTakeover = () => {
    runtime.userEpoch += 1;
    runtime.refs = new Map();
    runtime.agentCursor?.remove();
    runtime.agentCursor = null;
    runtime.cursorDocument = null;
    runtime.cursorPoint = null;
  };
  runtime.previewAction = (input) => {
    const { el, end } = validateAction(input);
    const destination = input.action === 'drag' ? end : el;
    return { durationMs: moveAgentCursor(destination, input.action === 'drag' ? el : null), action: input.action };
  };
  runtime.validateAction = (input) => { validateAction(input); return true; };
  runtime.prepareNativePointer = (input) => {
    const { el } = validateAction(input);
    if (!el) throw new Error('Browser pointer action requires a target');
    const ownerDocument = el.ownerDocument || document;
    const ownerWindow = ownerDocument.defaultView || window;
    let rect = el.getBoundingClientRect();
    if (rect.top < 0 || rect.left < 0 || rect.bottom > ownerWindow.innerHeight || rect.right > ownerWindow.innerWidth) {
      el.scrollIntoView({ behavior: 'instant', block: 'center', inline: 'center' });
      rect = el.getBoundingClientRect();
    }
    const point = { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
    if (point.x < 0 || point.y < 0 || point.x >= ownerWindow.innerWidth || point.y >= ownerWindow.innerHeight) {
      throw new Error('Browser pointer target is outside the viewport');
    }
    const hit = ownerDocument.elementFromPoint(point.x, point.y);
    if (!hit || (hit !== el && !el.contains(hit))) {
      throw new Error('Browser pointer target is covered by another element');
    }
    return {
      bounds: viewportBoundsOf(el),
      verificationBaseline: actionVerificationBaseline(),
    };
  };
  runtime.prepareTrustedText = (input) => {
    const { el } = validateAction(input);
    if (!el) throw new Error('Trusted browser text input requires a target');
    const ownerDocument = el.ownerDocument || document;
    const ownerWindow = ownerDocument.defaultView || window;
    const editable = el.isContentEditable
      || el instanceof ownerWindow.HTMLInputElement
      || el instanceof ownerWindow.HTMLTextAreaElement;
    if (!editable) throw new Error('Trusted browser text input requires an editable target');
    el.focus();
    if (typeof el.select === 'function') {
      el.select();
    } else {
      const range = ownerDocument.createRange();
      range.selectNodeContents(el);
      const selection = ownerWindow.getSelection();
      selection?.removeAllRanges();
      selection?.addRange(range);
    }
    return {
      focused: ownerDocument.activeElement === el,
      verificationBaseline: actionVerificationBaseline(),
    };
  };
  runtime.prepareTrustedKey = (input) => {
    const { el } = validateAction(input);
    const ownerDocument = el?.ownerDocument || document;
    const target = el || ownerDocument.activeElement || ownerDocument.body;
    target?.focus?.();
    return {
      focused: ownerDocument.activeElement === target,
      verificationBaseline: actionVerificationBaseline(),
    };
  };
  runtime.act = (input) => {
    const { el, end } = validateAction(input);
    runtime.synthetic = true;
    try {
      if (input.action === 'move' || input.action === 'hover') hoverAt(el, input, centerOf(el));
      else if (input.action === 'click') clickAt(el, input);
      else if (input.action === 'double_click') {
        clickAt(el, input, 1);
        clickAt(el, input, 2);
        dispatchPointer(el, 'dblclick', input, centerOf(el), 2);
      } else if (input.action === 'drag') dragBetween(el, end, input);
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
  const pickMessageMarker = '__NEXA_BROWSER_PICK_BRIDGE__';
  const pickMessageToken = __NEXA_PICK_TOKEN__;
  const apply = Reflect.apply;
  const nativePostMessage = Window.prototype.postMessage;
  const nativeStopImmediatePropagation = Event.prototype.stopImmediatePropagation;
  const postPickMessage = (target, message) => {
    apply(nativePostMessage, target, [{ marker: pickMessageMarker, token: pickMessageToken, ...message }, '*']);
  };
  const applyPickMode = (mode) => {
    runtime.pickMode = mode;
    runtime.pendingArtifact = null;
    runtime.regionStart = null;
    clearOverlay();
  };
  const childFrames = () => Array.from(document.querySelectorAll('iframe'))
    .map((frame) => ({ frame, target: frame.contentWindow }))
    .filter(({ target }) => Boolean(target));
  const broadcastPick = (message) => {
    for (const { target } of childFrames()) {
      try { postPickMessage(target, message); } catch (_) {}
    }
  };
  const publishArtifact = (artifact) => {
    applyPickMode(null);
    if (window === window.top) {
      runtime.pendingArtifact = artifact;
      broadcastPick({ kind: 'cancel' });
    } else {
      postPickMessage(window.parent, { kind: 'artifact', artifact });
    }
  };
  runtime.beginPick = (mode) => { applyPickMode(mode); broadcastPick({ kind: 'begin', mode }); };
  runtime.cancelPick = () => { applyPickMode(null); broadcastPick({ kind: 'cancel' }); };
  runtime.takeArtifact = () => { const artifact = runtime.pendingArtifact; runtime.pendingArtifact = null; return artifact; };
  runtime.selectedText = () => String(window.getSelection?.() || '').trim().slice(0, 10000);

  addEventListener('message', (event) => {
    const message = event.data;
    if (!message || message.marker !== pickMessageMarker || message.token !== pickMessageToken) return;
    apply(nativeStopImmediatePropagation, event, []);
    if (event.source === window.parent && window !== window.top) {
      if (message.kind === 'begin') {
        applyPickMode(message.mode);
        broadcastPick({ kind: 'begin', mode: message.mode });
      } else if (message.kind === 'cancel') {
        applyPickMode(null);
        broadcastPick({ kind: 'cancel' });
      }
      return;
    }
    if (message.kind !== 'artifact') return;
    const child = childFrames().find(({ target }) => target === event.source);
    if (!child || !message.artifact) return;
    const artifact = { ...message.artifact };
    if (artifact.bounds) {
      const frameBounds = child.frame.getBoundingClientRect();
      artifact.bounds = {
        ...artifact.bounds,
        x: Number(artifact.bounds.x || 0) + frameBounds.x,
        y: Number(artifact.bounds.y || 0) + frameBounds.y,
      };
    }
    if (window === window.top) {
      applyPickMode(null);
      runtime.pendingArtifact = artifact;
      broadcastPick({ kind: 'cancel' });
    } else {
      postPickMessage(window.parent, { kind: 'artifact', artifact });
    }
  }, true);
  addEventListener('load', (event) => {
    const frame = event.target;
    if (!runtime.pickMode || String(frame?.tagName || '').toUpperCase() !== 'IFRAME') return;
    try { if (frame.contentWindow) postPickMessage(frame.contentWindow, { kind: 'begin', mode: runtime.pickMode }); } catch (_) {}
  }, true);

  addEventListener('pointermove', (event) => {
    if (runtime.pickMode !== 'element') return;
    const target = event.composedPath?.().find((item) => item instanceof Element);
    if (target) showOverlay(target.getBoundingClientRect(), `${roleOf(target)} ${textOf(target)}`);
  }, true);
  addEventListener('pointerdown', (event) => {
    if (
      !runtime.synthetic
      && event.isTrusted
      && !window.__NEXA_TRUSTED_INPUT_GUARD__?.expects('pointerdown', event)
    ) {
      runtime.userEpoch += 1;
    }
    if (runtime.pickMode === 'region') {
      if (!event.isTrusted) return;
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
    if (!event.isTrusted) return;
    const bounds = { x: Math.min(runtime.regionStart.x, event.clientX), y: Math.min(runtime.regionStart.y, event.clientY), width: Math.abs(event.clientX - runtime.regionStart.x), height: Math.abs(event.clientY - runtime.regionStart.y) };
    publishArtifact({ kind: 'region', capture: 'coordinatesOnly', url: location.href, title: document.title, bounds, userEpoch: runtime.userEpoch });
    event.preventDefault(); event.stopImmediatePropagation();
  }, true);
  addEventListener('click', (event) => {
    if (runtime.pickMode !== 'element') return;
    if (!event.isTrusted) return;
    const target = event.composedPath?.().find((item) => item instanceof Element);
    if (target) {
      const ref = `picked_${Date.now()}`;
      runtime.refs.set(ref, target);
      publishArtifact({ kind: 'element', url: location.href, title: document.title, userEpoch: runtime.userEpoch, ...describe(target, ref) });
    }
    if (!target) runtime.cancelPick();
    event.preventDefault(); event.stopImmediatePropagation();
  }, true);
  addEventListener('keydown', (event) => {
    if (
      !runtime.synthetic
      && event.isTrusted
      && !window.__NEXA_TRUSTED_INPUT_GUARD__?.expects('keydown', event)
    ) {
      runtime.userEpoch += 1;
    }
    if (event.key === 'Escape' && runtime.pickMode) { runtime.cancelPick(); event.preventDefault(); event.stopImmediatePropagation(); }
  }, true);
  addEventListener('input', (event) => {
    if (
      !runtime.synthetic
      && event.isTrusted
      && !window.__NEXA_TRUSTED_INPUT_GUARD__?.expects('input', event)
    ) {
      runtime.userEpoch += 1;
    }
  }, true);
  addEventListener('wheel', (event) => {
    if (!runtime.synthetic && event.isTrusted) {
      runtime.userEpoch += 1;
    }
  }, true);
  const bridge = Object.freeze({
    observe: () => runtime.observe(),
    previewAction: (input) => runtime.previewAction(input),
    validateAction: (input) => runtime.validateAction(input),
    prepareNativePointer: (input) => runtime.prepareNativePointer(input),
    prepareTrustedText: (input) => runtime.prepareTrustedText(input),
    prepareTrustedKey: (input) => runtime.prepareTrustedKey(input),
    act: (input) => runtime.act(input),
    invalidateForUserTakeover: () => runtime.invalidateForUserTakeover(),
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
