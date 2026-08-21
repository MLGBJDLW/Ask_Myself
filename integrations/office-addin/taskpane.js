const statusElement = document.querySelector('#status');
const endpointElement = document.querySelector('#endpoint');
const pairingElement = document.querySelector('#pairing');
const connectButton = document.querySelector('#connect');

let officeHost = null;
let endpoint = null;
let bridgeToken = null;
let sessionId = null;
let polling = false;

function status(message) {
  statusElement.textContent = message;
}

function validateEndpoint(value) {
  const parsed = new URL(value);
  if (parsed.protocol !== 'http:' || parsed.hostname !== '127.0.0.1' || !parsed.port || parsed.pathname !== '/') {
    throw new Error('Endpoint must be the exact http://127.0.0.1:<port> value shown by Nexa.');
  }
  return parsed.origin;
}

async function bridgeRequest(route, payload, token = null) {
  const response = await fetch(`${endpoint}${route}`, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      ...(token ? { authorization: `Bearer ${token}` } : {}),
    },
    body: JSON.stringify(payload),
    cache: 'no-store',
  });
  const body = await response.json();
  if (!response.ok) throw new Error(body.error ?? `Bridge request failed (${response.status})`);
  return body;
}

function requirementSets(host) {
  const candidates = host === 'Word'
    ? [['WordApi', '1.4']]
    : host === 'Excel'
      ? [['ExcelApi', '1.13']]
      : [['PowerPointApi', '1.4'], ['PowerPointApi', '1.3']];
  return candidates
    .filter(([name, version]) => Office.context.requirements.isSetSupported(name, version))
    .map(([name, version]) => `${name}:${version}`);
}

function capabilities(host, sets) {
  if (host === 'Word') {
    return [
      'word.replace-text',
      'word.insert-text',
      ...(sets.includes('WordApi:1.4') ? ['word.add-comment'] : []),
    ];
  }
  if (host === 'Excel') {
    return ['excel.set-range', 'excel.set-formula', 'excel.format-range'];
  }
  return [
    ...(sets.includes('PowerPointApi:1.4') ? ['powerpoint.set-text'] : []),
    ...(sets.includes('PowerPointApi:1.3') ? ['powerpoint.add-slide'] : []),
  ];
}

async function executeWord(operation) {
  return Word.run(async (context) => {
    if (operation.op === 'word_replace_text') {
      const matches = context.document.body.search(operation.search, {
        matchCase: Boolean(operation.matchCase),
        matchWholeWord: Boolean(operation.matchWholeWord),
      });
      matches.load('items');
      await context.sync();
      for (const match of matches.items) match.insertText(operation.replacement, 'Replace');
      await context.sync();
      return { matches: matches.items.length };
    }
    if (operation.op === 'word_insert_text') {
      const target = operation.location === 'selection'
        ? context.document.getSelection()
        : context.document.body;
      const location = operation.location === 'start' ? 'Start' : operation.location === 'end' ? 'End' : 'Replace';
      target.insertText(operation.text, location);
      await context.sync();
      return { inserted: true, location: operation.location };
    }
    if (operation.op === 'word_add_comment') {
      if (!Office.context.requirements.isSetSupported('WordApi', '1.4')) throw new Error('WordApi 1.4 is required');
      const matches = context.document.body.search(operation.search);
      matches.load('items');
      await context.sync();
      if (matches.items.length !== 1) throw new Error(`Comment anchor must match exactly once; found ${matches.items.length}`);
      matches.items[0].insertComment(operation.comment);
      await context.sync();
      return { commentAdded: true };
    }
    throw new Error(`Unsupported Word operation: ${operation.op}`);
  });
}

function excelWorksheet(context, name) {
  return name
    ? context.workbook.worksheets.getItem(name)
    : context.workbook.worksheets.getActiveWorksheet();
}

async function executeExcel(operation) {
  return Excel.run(async (context) => {
    const range = excelWorksheet(context, operation.sheet).getRange(operation.address);
    if (operation.op === 'excel_set_range') {
      range.values = operation.values;
    } else if (operation.op === 'excel_set_formula') {
      range.formulas = operation.formulas;
    } else if (operation.op === 'excel_format_range') {
      const format = operation.format ?? {};
      const allowed = new Set(['fillColor', 'fontColor', 'fontBold', 'columnWidth', 'rowHeight', 'numberFormat']);
      for (const key of Object.keys(format)) if (!allowed.has(key)) throw new Error(`Unsupported Excel format key: ${key}`);
      if (format.fillColor) range.format.fill.color = String(format.fillColor);
      if (format.fontColor) range.format.font.color = String(format.fontColor);
      if (typeof format.fontBold === 'boolean') range.format.font.bold = format.fontBold;
      if (typeof format.columnWidth === 'number') range.format.columnWidth = format.columnWidth;
      if (typeof format.rowHeight === 'number') range.format.rowHeight = format.rowHeight;
      if (Array.isArray(format.numberFormat)) range.numberFormat = format.numberFormat;
    } else {
      throw new Error(`Unsupported Excel operation: ${operation.op}`);
    }
    await context.sync();
    return { address: operation.address, updated: true };
  });
}

async function powerpointSlide(context, operation) {
  if (operation.slideId) return context.presentation.slides.getItem(operation.slideId);
  if (!Number.isInteger(operation.slideIndex) || operation.slideIndex < 1) throw new Error('slideId or 1-based slideIndex is required');
  return context.presentation.slides.getItemAt(operation.slideIndex - 1);
}

async function executePowerPoint(operation) {
  return PowerPoint.run(async (context) => {
    if (operation.op === 'powerpoint_add_slide') {
      if (!Office.context.requirements.isSetSupported('PowerPointApi', '1.3')) throw new Error('PowerPointApi 1.3 is required');
      const options = operation.layoutId ? { layoutId: operation.layoutId } : undefined;
      const slide = context.presentation.slides.add(options);
      if (operation.title) {
        const title = slide.shapes.addTextBox(operation.title, { left: 40, top: 28, width: 600, height: 48 });
        title.name = 'Nexa live title';
      }
      await context.sync();
      return { added: true };
    }
    if (operation.op === 'powerpoint_set_text') {
      if (!Office.context.requirements.isSetSupported('PowerPointApi', '1.4')) throw new Error('PowerPointApi 1.4 is required');
      const slide = await powerpointSlide(context, operation);
      let shape;
      if (operation.shapeId) {
        shape = slide.shapes.getItem(operation.shapeId);
      } else {
        slide.shapes.load('items/id,name');
        await context.sync();
        const matched = slide.shapes.items.filter((item) => item.name === operation.shapeName);
        if (matched.length !== 1) throw new Error(`Shape name must match exactly once; found ${matched.length}`);
        shape = slide.shapes.getItem(matched[0].id);
      }
      shape.textFrame.textRange.text = operation.text;
      await context.sync();
      return { updated: true };
    }
    throw new Error(`Unsupported PowerPoint operation: ${operation.op}`);
  });
}

async function executeOperation(operation) {
  if (officeHost === 'Word') return executeWord(operation);
  if (officeHost === 'Excel') return executeExcel(operation);
  if (officeHost === 'PowerPoint') return executePowerPoint(operation);
  throw new Error(`Unsupported Office host: ${officeHost}`);
}

async function poll() {
  if (polling || !sessionId) return;
  polling = true;
  try {
    const envelope = await bridgeRequest('/v1/poll', { sessionId }, bridgeToken);
    const queued = envelope.operation;
    if (queued) {
      let result;
      try {
        if (!Number.isFinite(queued.deadlineAt) || Math.floor(Date.now() / 1000) >= queued.deadlineAt) {
          throw new Error('Operation lease expired before Office mutation');
        }
        result = { status: 'ok', result: await executeOperation(queued.operation), error: null };
      } catch (error) {
        result = { status: 'error', result: {}, error: error instanceof Error ? error.message : String(error) };
      }
      await bridgeRequest('/v1/result', {
        operationId: queued.operationId,
        sessionId,
        ...result,
      }, bridgeToken);
      status(`Connected to ${officeHost}. Last operation: ${queued.operation.op} (${result.status}).`);
    }
  } catch (error) {
    status(`Bridge error: ${error instanceof Error ? error.message : String(error)}`);
  } finally {
    polling = false;
  }
}

async function pairAndRegister() {
  if (!officeHost) throw new Error('Office is not ready');
  endpoint = validateEndpoint(endpointElement.value.trim());
  const pairingCode = pairingElement.value.trim();
  if (!/^\d{6}$/.test(pairingCode)) throw new Error('Pairing code must contain six digits');
  const paired = await bridgeRequest('/v1/pair', { pairingCode });
  bridgeToken = paired.bridgeToken;
  const sets = requirementSets(officeHost);
  const registered = await bridgeRequest('/v1/register', {
    host: officeHost,
    documentId: Office.context.document.url || `${officeHost}-active-document`,
    requirementSets: sets,
    capabilities: capabilities(officeHost, sets),
  }, bridgeToken);
  sessionId = registered.session.sessionId;
  status(`Connected to ${officeHost}. Session ${sessionId}. Keep this pane open.`);
  window.setInterval(poll, 700);
}

connectButton.addEventListener('click', () => {
  pairAndRegister().catch((error) => status(error instanceof Error ? error.message : String(error)));
});

Office.onReady((info) => {
  officeHost = info.host;
  status(`Office ready: ${officeHost}. Enter the Nexa pairing details.`);
});
