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

function hasOwn(value, key) {
  return Object.prototype.hasOwnProperty.call(value, key);
}

function validateEndpoint(value) {
  const parsed = new URL(value);
  if (!['http:', 'https:'].includes(parsed.protocol) || parsed.hostname !== '127.0.0.1' || !parsed.port || parsed.pathname !== '/' || parsed.search || parsed.hash || parsed.username || parsed.password) {
    throw new Error('Endpoint must be the exact http(s)://127.0.0.1:<port> value shown by Nexa.');
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
    return sets.includes('WordApi:1.4')
      ? [
        'word.replace-text',
        'word.insert-text',
        'word.add-comment',
        'word.set-change-tracking',
        'word.wrap-content-control',
        'word.reply-comment',
        'word.resolve-comment',
      ]
      : [];
  }
  if (host === 'Excel') {
    return sets.includes('ExcelApi:1.13')
      ? [
        'excel.set-range',
        'excel.set-formula',
        'excel.format-range',
        'excel.create-table',
        'excel.add-chart',
        'excel.calculate',
      ]
      : [];
  }
  return [
    ...(sets.includes('PowerPointApi:1.4') ? ['powerpoint.set-text'] : []),
    ...(sets.includes('PowerPointApi:1.4') ? ['powerpoint.add-textbox', 'powerpoint.add-shape'] : []),
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
    if (operation.op === 'word_set_change_tracking') {
      if (!Office.context.requirements.isSetSupported('WordApi', '1.4')) throw new Error('WordApi 1.4 is required');
      const modes = {
        off: Word.ChangeTrackingMode.off,
        track_all: Word.ChangeTrackingMode.trackAll,
        track_mine_only: Word.ChangeTrackingMode.trackMineOnly,
      };
      if (!hasOwn(modes, operation.mode)) throw new Error(`Unsupported change tracking mode: ${operation.mode}`);
      context.document.changeTrackingMode = modes[operation.mode];
      await context.sync();
      return { changeTrackingMode: operation.mode };
    }
    if (operation.op === 'word_wrap_content_control') {
      if (!Office.context.requirements.isSetSupported('WordApi', '1.4')) throw new Error('WordApi 1.4 is required');
      const matches = context.document.body.search(operation.search);
      matches.load('items');
      await context.sync();
      if (matches.items.length !== 1) throw new Error(`Content-control anchor must match exactly once; found ${matches.items.length}`);
      const control = matches.items[0].insertContentControl();
      control.tag = operation.tag;
      if (operation.title) control.title = operation.title;
      control.load('id,tag,title');
      await context.sync();
      return { contentControlId: control.id, tag: control.tag, title: control.title };
    }
    if (operation.op === 'word_reply_comment' || operation.op === 'word_resolve_comment') {
      if (!Office.context.requirements.isSetSupported('WordApi', '1.4')) throw new Error('WordApi 1.4 is required');
      const comments = context.document.body.getComments();
      comments.load('items/id');
      await context.sync();
      const matched = comments.items.filter((item) => item.id === operation.commentId);
      if (matched.length !== 1) throw new Error(`commentId must match exactly once; found ${matched.length}`);
      if (operation.op === 'word_reply_comment') matched[0].reply(operation.comment);
      else matched[0].resolved = operation.resolved;
      await context.sync();
      return operation.op === 'word_reply_comment'
        ? { commentId: operation.commentId, replied: true }
        : { commentId: operation.commentId, resolved: operation.resolved };
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
    if (operation.op === 'excel_set_range') {
      const range = excelWorksheet(context, operation.sheet).getRange(operation.address);
      range.values = operation.values;
    } else if (operation.op === 'excel_set_formula') {
      const range = excelWorksheet(context, operation.sheet).getRange(operation.address);
      range.formulas = operation.formulas;
    } else if (operation.op === 'excel_format_range') {
      const range = excelWorksheet(context, operation.sheet).getRange(operation.address);
      const format = operation.format ?? {};
      const allowed = new Set(['fillColor', 'fontColor', 'fontBold', 'columnWidth', 'rowHeight', 'numberFormat']);
      for (const key of Object.keys(format)) if (!allowed.has(key)) throw new Error(`Unsupported Excel format key: ${key}`);
      if (hasOwn(format, 'fillColor') && typeof format.fillColor !== 'string') throw new Error('fillColor must be a string');
      if (hasOwn(format, 'fontColor') && typeof format.fontColor !== 'string') throw new Error('fontColor must be a string');
      if (hasOwn(format, 'fontBold') && typeof format.fontBold !== 'boolean') throw new Error('fontBold must be a boolean');
      if (hasOwn(format, 'columnWidth') && (!Number.isFinite(format.columnWidth) || format.columnWidth <= 0 || format.columnWidth > 1000)) throw new Error('columnWidth must be in (0, 1000]');
      if (hasOwn(format, 'rowHeight') && (!Number.isFinite(format.rowHeight) || format.rowHeight <= 0 || format.rowHeight > 1000)) throw new Error('rowHeight must be in (0, 1000]');
      if (hasOwn(format, 'numberFormat') && (!Array.isArray(format.numberFormat) || !format.numberFormat.every((row) => Array.isArray(row) && row.every((cell) => typeof cell === 'string')))) throw new Error('numberFormat must be a string matrix');
      if (hasOwn(format, 'fillColor')) range.format.fill.color = format.fillColor;
      if (hasOwn(format, 'fontColor')) range.format.font.color = format.fontColor;
      if (hasOwn(format, 'fontBold')) range.format.font.bold = format.fontBold;
      if (hasOwn(format, 'columnWidth')) range.format.columnWidth = format.columnWidth;
      if (hasOwn(format, 'rowHeight')) range.format.rowHeight = format.rowHeight;
      if (hasOwn(format, 'numberFormat')) range.numberFormat = format.numberFormat;
    } else if (operation.op === 'excel_create_table') {
      const sheet = excelWorksheet(context, operation.sheet);
      const table = context.workbook.tables.add(sheet.getRange(operation.address), operation.hasHeaders);
      if (operation.name) table.name = operation.name;
      table.load('name');
      await context.sync();
      return { tableName: table.name, address: operation.address };
    } else if (operation.op === 'excel_add_chart') {
      const sheet = excelWorksheet(context, operation.sheet);
      const chartTypes = {
        column_clustered: Excel.ChartType.columnClustered,
        bar_clustered: Excel.ChartType.barClustered,
        line: Excel.ChartType.line,
        line_markers: Excel.ChartType.lineMarkers,
        area: Excel.ChartType.area,
        pie: Excel.ChartType.pie,
        doughnut: Excel.ChartType.doughnut,
      };
      const seriesBy = {
        auto: Excel.ChartSeriesBy.auto,
        columns: Excel.ChartSeriesBy.columns,
        rows: Excel.ChartSeriesBy.rows,
      };
      if (!hasOwn(chartTypes, operation.chartType)) throw new Error(`Unsupported Excel chart type: ${operation.chartType}`);
      if (operation.seriesBy && !hasOwn(seriesBy, operation.seriesBy)) throw new Error(`Unsupported seriesBy value: ${operation.seriesBy}`);
      const chart = sheet.charts.add(
        chartTypes[operation.chartType],
        sheet.getRange(operation.sourceAddress),
        operation.seriesBy ? seriesBy[operation.seriesBy] : Excel.ChartSeriesBy.auto,
      );
      if (operation.name) chart.name = operation.name;
      if (operation.title) chart.title.text = operation.title;
      if (operation.positionStart) chart.setPosition(operation.positionStart, operation.positionEnd ?? operation.positionStart);
      chart.load('name');
      await context.sync();
      return { chartName: chart.name, sourceAddress: operation.sourceAddress };
    } else if (operation.op === 'excel_calculate') {
      const calculationTypes = {
        recalculate: Excel.CalculationType.recalculate,
        full: Excel.CalculationType.full,
        full_rebuild: Excel.CalculationType.fullRebuild,
      };
      if (!hasOwn(calculationTypes, operation.calculationType)) throw new Error(`Unsupported calculation type: ${operation.calculationType}`);
      context.workbook.application.calculate(calculationTypes[operation.calculationType]);
    } else {
      throw new Error(`Unsupported Excel operation: ${operation.op}`);
    }
    await context.sync();
    return { address: operation.address ?? null, updated: true };
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
    if (operation.op === 'powerpoint_add_textbox' || operation.op === 'powerpoint_add_shape') {
      if (!Office.context.requirements.isSetSupported('PowerPointApi', '1.4')) throw new Error('PowerPointApi 1.4 is required');
      if (![operation.left, operation.top, operation.width, operation.height].every(Number.isFinite) || operation.width <= 0 || operation.height <= 0) {
        throw new Error('PowerPoint shape geometry must contain finite coordinates and positive dimensions');
      }
      const slide = await powerpointSlide(context, operation);
      const options = { left: operation.left, top: operation.top, width: operation.width, height: operation.height };
      let shape;
      if (operation.op === 'powerpoint_add_textbox') {
        shape = slide.shapes.addTextBox(operation.text, options);
      } else {
        const shapeTypes = {
          rectangle: PowerPoint.GeometricShapeType.rectangle,
          round_rectangle: PowerPoint.GeometricShapeType.roundRectangle,
          ellipse: PowerPoint.GeometricShapeType.ellipse,
          triangle: PowerPoint.GeometricShapeType.triangle,
          diamond: PowerPoint.GeometricShapeType.diamond,
          hexagon: PowerPoint.GeometricShapeType.hexagon,
          chevron: PowerPoint.GeometricShapeType.chevron,
          right_arrow: PowerPoint.GeometricShapeType.rightArrow,
          flow_chart_process: PowerPoint.GeometricShapeType.flowChartProcess,
          flow_chart_decision: PowerPoint.GeometricShapeType.flowChartDecision,
        };
        if (!hasOwn(shapeTypes, operation.shapeType)) throw new Error(`Unsupported PowerPoint shape type: ${operation.shapeType}`);
        shape = slide.shapes.addGeometricShape(shapeTypes[operation.shapeType], options);
        if (operation.text) shape.textFrame.textRange.text = operation.text;
        if (operation.fillColor) shape.fill.setSolidColor(operation.fillColor);
        if (operation.fontColor) shape.textFrame.textRange.font.color = operation.fontColor;
        if (typeof operation.fontBold === 'boolean') shape.textFrame.textRange.font.bold = operation.fontBold;
      }
      if (operation.name) shape.name = operation.name;
      shape.load('id,name');
      await context.sync();
      return { shapeId: shape.id, shapeName: shape.name, added: true };
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
  const sets = requirementSets(officeHost);
  const hostCapabilities = capabilities(officeHost, sets);
  if (hostCapabilities.length === 0) {
    throw new Error(`${officeHost} does not expose the Office.js requirement set required by Nexa.`);
  }
  const paired = await bridgeRequest('/v1/pair', { pairingCode });
  bridgeToken = paired.bridgeToken;
  const registered = await bridgeRequest('/v1/register', {
    host: officeHost,
    documentId: Office.context.document.url || `${officeHost}-active-document`,
    requirementSets: sets,
    capabilities: hostCapabilities,
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
