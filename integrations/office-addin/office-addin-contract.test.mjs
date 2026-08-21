import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = path.dirname(fileURLToPath(import.meta.url));
const manifest = fs.readFileSync(path.join(root, 'manifest.xml'), 'utf8');
const taskpane = fs.readFileSync(path.join(root, 'taskpane.js'), 'utf8');

test('manifest covers all three hosts with read-write document permission', () => {
  for (const host of ['Document', 'Workbook', 'Presentation']) {
    assert.match(manifest, new RegExp(`<Host Name="${host}"`));
  }
  assert.match(manifest, /<Permissions>ReadWriteDocument<\/Permissions>/);
  assert.match(manifest, /https:\/\/localhost:3000\/taskpane\.html/);
});

test('taskpane implements the exact Rust live-operation union without eval escape hatches', () => {
  for (const operation of [
    'word_replace_text',
    'word_insert_text',
    'word_add_comment',
    'excel_set_range',
    'excel_set_formula',
    'excel_format_range',
    'powerpoint_set_text',
    'powerpoint_add_slide',
  ]) {
    assert.match(taskpane, new RegExp(`['"]${operation}['"]`));
  }
  assert.doesNotMatch(taskpane, /\beval\s*\(|new\s+Function\s*\(/);
  assert.match(taskpane, /parsed\.hostname !== '127\.0\.0\.1'/);
  assert.match(taskpane, /Office\.context\.requirements\.isSetSupported/);
  assert.match(taskpane, /authorization: `Bearer \$\{token\}`/);
});

test('Excel formatting is field-allowlisted and PowerPoint gates requirement sets', () => {
  assert.match(taskpane, /new Set\(\['fillColor', 'fontColor', 'fontBold', 'columnWidth', 'rowHeight', 'numberFormat'\]\)/);
  assert.match(taskpane, /PowerPointApi', '1\.4'/);
  assert.match(taskpane, /PowerPointApi', '1\.3'/);
  assert.match(taskpane, /WordApi', '1\.4'/);
});
