#!/usr/bin/env node
/** Network-closed, schema-bounded PptxGenJS author adapter. */

import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const configuredModuleRoot = process.env.NEXA_PPTXGENJS_MODULE_ROOT;
const pptxgenEntry = configuredModuleRoot
  ? path.join(path.resolve(configuredModuleRoot), "pptxgenjs")
  : "pptxgenjs";
const PptxGenJS = require(pptxgenEntry);

const MAX_SPEC_BYTES = 5 * 1024 * 1024;
const MAX_ASSET_BYTES = 50 * 1024 * 1024;
const ALLOWED_IMAGE_EXTENSIONS = new Set([".png", ".jpg", ".jpeg", ".gif", ".svg"]);
const ALLOWED_MEDIA_EXTENSIONS = new Set([".mp3", ".m4a", ".wav", ".mp4", ".mov"]);

function fail(message) {
  throw new Error(message);
}

function argumentsFrom(argv) {
  const out = {};
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (!flag?.startsWith("--") || value === undefined) fail("arguments must be --name value pairs");
    out[flag.slice(2)] = value;
  }
  return out;
}

function contained(root, candidate) {
  const relative = path.relative(root, candidate);
  return relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative));
}

function rejectSymlinkSegments(workspace, candidate, allowMissingTail = false) {
  const relative = path.relative(workspace, candidate);
  if (!contained(workspace, candidate)) fail("path escapes workspace");
  let current = workspace;
  for (const segment of relative.split(path.sep).filter(Boolean)) {
    current = path.join(current, segment);
    if (!fs.existsSync(current)) {
      if (allowMissingTail) return;
      fail(`workspace path is missing: ${current}`);
    }
    if (fs.lstatSync(current).isSymbolicLink()) fail(`workspace path contains a symbolic link: ${current}`);
  }
}

function localAsset(raw, workspace, extensions) {
  if (typeof raw !== "string" || /^(?:https?|file|data):/i.test(raw) || raw.startsWith("\\\\")) {
    fail("assets must be reviewed local workspace files; URLs, data URIs, and UNC paths are blocked");
  }
  const resolved = path.resolve(workspace, raw);
  if (!contained(workspace, resolved)) fail("asset escapes workspace");
  rejectSymlinkSegments(workspace, resolved);
  const real = fs.realpathSync.native(resolved);
  if (!contained(workspace, real)) fail("asset real path escapes workspace");
  const extension = path.extname(resolved).toLowerCase();
  if (!extensions.has(extension)) fail(`unsupported asset extension: ${extension}`);
  const stat = fs.lstatSync(real);
  if (!stat.isFile() || stat.size > MAX_ASSET_BYTES) fail("asset is missing, not a file, or exceeds 50 MiB");
  return real;
}

function geometry(element) {
  for (const key of ["x", "y", "w", "h"]) {
    if (typeof element[key] !== "number" || !Number.isFinite(element[key]) || element[key] < 0) {
      fail(`element.${key} must be a finite non-negative number`);
    }
  }
  return { x: element.x, y: element.y, w: element.w, h: element.h };
}

function chartType(pptx, name) {
  const map = {
    bar: pptx.ChartType.bar,
    column: pptx.ChartType.bar,
    line: pptx.ChartType.line,
    pie: pptx.ChartType.pie,
    doughnut: pptx.ChartType.doughnut,
    scatter: pptx.ChartType.scatter,
    bubble: pptx.ChartType.bubble,
    area: pptx.ChartType.area,
    radar: pptx.ChartType.radar,
  };
  if (!map[name]) fail(`unsupported chart type: ${name}`);
  return map[name];
}

function chartData(value) {
  if (!Array.isArray(value) || value.length === 0) fail("chart data must be a non-empty series array");
  return value.map((series) => {
    if (typeof series?.name !== "string" || !Array.isArray(series.labels) || !Array.isArray(series.values)) {
      fail("each chart series requires name, labels, and values arrays");
    }
    if (series.labels.length !== series.values.length || series.values.some((item) => typeof item !== "number")) {
      fail("chart labels and numeric values must have equal lengths");
    }
    return { name: series.name, labels: series.labels.map(String), values: series.values };
  });
}

function validateMaster(master) {
  if (typeof master?.title !== "string" || !Array.isArray(master.objects)) fail("master requires title and objects");
  for (const object of master.objects) {
    const keys = Object.keys(object ?? {});
    if (keys.length !== 1 || !["text", "rect", "line", "placeholder"].includes(keys[0])) {
      fail("master objects are limited to text, rect, line, and placeholder");
    }
    const serialized = JSON.stringify(object);
    if (/"(?:path|data|url)"\s*:/i.test(serialized) || /(?:https?|file):\/\//i.test(serialized) || serialized.includes("\\\\")) {
      fail("master objects cannot contain external or unreviewed asset references");
    }
  }
  return master;
}

function addElement(pptx, slide, element, workspace) {
  const box = geometry(element);
  const options = { ...box, ...(element.options ?? {}) };
  switch (element.type) {
    case "text":
      slide.addText(element.runs ?? String(element.text ?? ""), options);
      return;
    case "shape": {
      const shape = pptx.ShapeType[element.shape ?? "rect"];
      if (!shape) fail(`unsupported shape: ${element.shape}`);
      slide.addShape(shape, options);
      return;
    }
    case "image": {
      if (typeof element.altText !== "string" || element.altText.trim() === "") fail("image altText is required");
      const imagePath = localAsset(element.path, workspace, ALLOWED_IMAGE_EXTENSIONS);
      slide.addImage({ path: imagePath, altText: element.altText, ...options });
      return;
    }
    case "svg": {
      if (typeof element.altText !== "string" || element.altText.trim() === "") fail("SVG altText is required");
      const svgPath = localAsset(element.path, workspace, new Set([".svg"]));
      const encoded = Buffer.from(fs.readFileSync(svgPath, "utf8"), "utf8").toString("base64");
      slide.addImage({ data: `data:image/svg+xml;base64,${encoded}`, altText: element.altText, ...options });
      return;
    }
    case "table":
      if (!Array.isArray(element.rows) || element.rows.length === 0) fail("table rows are required");
      slide.addTable(element.rows, options);
      return;
    case "chart": {
      if (typeof element.altText !== "string" || element.altText.trim() === "") fail("chart altText is required");
      options.altText = element.altText;
      if (element.chartType === "combo") {
        if (!Array.isArray(element.series) || element.series.length < 2) fail("combo chart requires at least two typed series groups");
        const groups = element.series.map((group) => ({
          type: chartType(pptx, group.type),
          data: chartData(group.data),
          options: group.options ?? {},
        }));
        slide.addChart(groups, options);
      } else {
        slide.addChart(chartType(pptx, element.chartType), chartData(element.data), options);
      }
      return;
    }
    case "media": {
      const mediaPath = localAsset(element.path, workspace, ALLOWED_MEDIA_EXTENSIONS);
      const mediaType = ["audio", "video"].includes(element.mediaType) ? element.mediaType : fail("mediaType must be audio or video");
      slide.addMedia({ type: mediaType, path: mediaPath, ...options });
      return;
    }
    default:
      fail(`unsupported element type: ${element.type}`);
  }
}

async function main() {
  const args = argumentsFrom(process.argv.slice(2));
  if (!args.spec || !args.out || !args.workspace) fail("--spec, --out, and --workspace are required");
  const workspaceArgument = path.resolve(args.workspace);
  const workspace = fs.realpathSync.native(workspaceArgument);
  if (!fs.lstatSync(workspace).isDirectory()) fail("workspace must be a real directory");
  const specPath = localAsset(args.spec, workspace, new Set([".json"]));
  if (fs.statSync(specPath).size > MAX_SPEC_BYTES) fail("spec exceeds 5 MiB");
  const spec = JSON.parse(fs.readFileSync(specPath, "utf8"));
  if (spec?.schemaVersion !== 1 || !Array.isArray(spec.slides) || spec.slides.length === 0) {
    fail("PptxGenJS spec requires schemaVersion=1 and non-empty slides");
  }
  const output = path.resolve(args.out);
  if (!contained(workspace, output) || path.extname(output).toLowerCase() !== ".pptx") {
    fail("output must be a .pptx inside workspace");
  }
  rejectSymlinkSegments(workspace, output, true);
  let existingParent = path.dirname(output);
  while (!fs.existsSync(existingParent)) existingParent = path.dirname(existingParent);
  const realParent = fs.realpathSync.native(existingParent);
  if (!contained(workspace, realParent)) fail("output parent real path escapes workspace");

  const pptx = new PptxGenJS();
  pptx.layout = spec.layout ?? "LAYOUT_WIDE";
  pptx.author = String(spec.author ?? "Nexa");
  pptx.company = String(spec.company ?? "Nexa");
  pptx.subject = String(spec.subject ?? "");
  pptx.title = String(spec.title ?? "");
  pptx.lang = String(spec.lang ?? "zh-CN");
  if (spec.theme) pptx.theme = spec.theme;
  for (const master of spec.masters ?? []) {
    pptx.defineSlideMaster(validateMaster(master));
  }
  for (const slideSpec of spec.slides) {
    const slide = slideSpec.masterName ? pptx.addSlide(slideSpec.masterName) : pptx.addSlide();
    if (slideSpec.background) slide.background = slideSpec.background;
    for (const element of slideSpec.elements ?? []) addElement(pptx, slide, element, workspace);
    if (slideSpec.notes) {
      if (!Array.isArray(slideSpec.notes) || !slideSpec.notes.every((item) => typeof item === "string")) fail("slide notes must be an array of strings");
      slide.addNotes(slideSpec.notes);
    }
  }
  fs.mkdirSync(path.dirname(output), { recursive: true });
  await pptx.writeFile({ fileName: output });
  process.stdout.write(JSON.stringify({
    kind: "pptxgenjsAuthorResult",
    engine: "pptxgenjs",
    engineVersion: "4.0.1",
    output,
    slides: spec.slides.length,
    editableNativeObjects: true,
    networkPolicy: "local-assets-only",
  }));
}

main().catch((error) => {
  process.stderr.write(`PPTXGENJS_FAILED: ${error.message}\n`);
  process.exitCode = 1;
});
