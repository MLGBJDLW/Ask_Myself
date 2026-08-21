import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = path.dirname(fileURLToPath(import.meta.url));
const manifest = fs.readFileSync(path.join(root, 'manifest.xml'), 'utf8');
const taskpane = fs.readFileSync(path.join(root, 'taskpane.js'), 'utf8');
const manifestTemplate = fs.readFileSync(path.join(root, 'manifest.template.xml'), 'utf8');
const manifestRenderer = fs.readFileSync(path.join(root, 'render_manifest.py'), 'utf8');
const httpsServer = fs.readFileSync(path.join(root, 'serve_https.py'), 'utf8');

test('manifest covers all three hosts with read-write document permission', () => {
  for (const host of ['Document', 'Workbook', 'Presentation']) {
    assert.match(manifest, new RegExp(`<Host Name="${host}"`));
  }
  assert.match(manifest, /<Permissions>ReadWriteDocument<\/Permissions>/);
  assert.match(manifest, /https:\/\/localhost:3000\/taskpane\.html/);
  assert.equal(manifestTemplate.replaceAll('{{ORIGIN}}', 'https://localhost:3000'), manifest);
});

test('taskpane implements the exact Rust live-operation union without eval escape hatches', () => {
  for (const operation of [
    'word_replace_text',
    'word_insert_text',
    'word_add_comment',
    'word_set_change_tracking',
    'word_wrap_content_control',
    'word_reply_comment',
    'word_resolve_comment',
    'excel_set_range',
    'excel_set_formula',
    'excel_format_range',
    'excel_create_table',
    'excel_add_chart',
    'excel_calculate',
    'powerpoint_set_text',
    'powerpoint_add_slide',
    'powerpoint_add_textbox',
    'powerpoint_add_shape',
  ]) {
    assert.match(taskpane, new RegExp(`['"]${operation}['"]`));
  }
  assert.doesNotMatch(taskpane, /\beval\s*\(|new\s+Function\s*\(/);
  assert.match(taskpane, /parsed\.hostname !== '127\.0\.0\.1'/);
  assert.match(taskpane, /Office\.context\.requirements\.isSetSupported/);
  assert.match(taskpane, /authorization: `Bearer \$\{token\}`/);
  assert.match(taskpane, /queued\.deadlineAt/);
  assert.match(taskpane, /Operation lease expired before Office mutation/);
  assert.match(taskpane, /changeTrackingMode/);
  assert.match(taskpane, /insertContentControl/);
  assert.match(taskpane, /getComments/);
  assert.match(taskpane, /workbook\.tables\.add/);
  assert.match(taskpane, /sheet\.charts\.add/);
  assert.match(taskpane, /application\.calculate/);
  assert.match(taskpane, /shapes\.addTextBox/);
  assert.match(taskpane, /shapes\.addGeometricShape/);
});

test('Excel formatting is field-allowlisted and PowerPoint gates requirement sets', () => {
  assert.match(taskpane, /new Set\(\['fillColor', 'fontColor', 'fontBold', 'columnWidth', 'rowHeight', 'numberFormat'\]\)/);
  assert.match(taskpane, /PowerPointApi', '1\.4'/);
  assert.match(taskpane, /PowerPointApi', '1\.3'/);
  assert.match(taskpane, /WordApi', '1\.4'/);
  assert.match(taskpane, /sets\.includes\('ExcelApi:1\.13'\)/);
});

test('deployment kit requires an exact trusted HTTPS origin and user-provided certificate', () => {
  assert.ok((manifestTemplate.match(/\{\{ORIGIN\}\}/g) ?? []).length >= 5);
  assert.match(manifestRenderer, /parsed\.scheme != "https"/);
  assert.match(manifestRenderer, /refusing to overwrite existing manifest without --force/);
  assert.match(httpsServer, /ssl\.PROTOCOL_TLS_SERVER/);
  assert.match(httpsServer, /context\.load_cert_chain/);
  assert.match(httpsServer, /TLSVersion\.TLSv1_2/);
  assert.match(httpsServer, /Content-Security-Policy/);
  assert.doesNotMatch(`${manifestRenderer}\n${httpsServer}`, /certutil|security add-trusted-cert|Import-Certificate|TrustRoot/);
});
