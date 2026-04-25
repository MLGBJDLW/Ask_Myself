#!/usr/bin/env node
import fs from "node:fs";

const file = process.argv[2] ?? "CHANGELOG.md";
const input = fs.readFileSync(file, "utf8");

const releaseHeadingPattern = /^## \[/gm;
const matches = [...input.matchAll(releaseHeadingPattern)];

function dedupeSection(section) {
  const lines = section.split("\n");
  const lastBulletIndexByTitle = new Map();

  lines.forEach((line, index) => {
    const match = line.match(/^\* (.+?) \(\[[0-9a-f]{7,}\]\(.+\)\)$/i);
    if (!match) return;
    lastBulletIndexByTitle.set(match[1].trim(), index);
  });

  const seen = new Set();
  return lines
    .filter((line, index) => {
      const match = line.match(/^\* (.+?) \(\[[0-9a-f]{7,}\]\(.+\)\)$/i);
      if (!match) return true;
      const title = match[1].trim();
      const shouldKeep = lastBulletIndexByTitle.get(title) === index;
      if (!shouldKeep || seen.has(title)) return false;
      seen.add(title);
      return true;
    })
    .join("\n");
}

let output = input;
if (matches.length === 0) {
  output = dedupeSection(input);
} else {
  for (let i = matches.length - 1; i >= 0; i -= 1) {
    const start = matches[i].index;
    const end = i + 1 < matches.length ? matches[i + 1].index : input.length;
    const section = input.slice(start, end);
    output = output.slice(0, start) + dedupeSection(section) + output.slice(end);
  }
}

if (output !== input) {
  fs.writeFileSync(file, output);
}
