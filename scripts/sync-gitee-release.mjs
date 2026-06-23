#!/usr/bin/env node
import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";

const required = ["GITHUB_TOKEN", "GITEE_TOKEN", "GITEE_OWNER", "GITEE_REPO", "RELEASE_TAG"];
for (const name of required) {
  if (!process.env[name]) {
    console.error(`::error::${name} is required.`);
    process.exit(1);
  }
}

const githubToken = process.env.GITHUB_TOKEN;
const giteeToken = process.env.GITEE_TOKEN;
const giteeOwner = process.env.GITEE_OWNER;
const giteeRepo = process.env.GITEE_REPO;
const giteeUsername = process.env.GITEE_USERNAME || giteeOwner;
const tag = process.env.RELEASE_TAG;
const githubRepository = process.env.GITHUB_REPOSITORY;

if (!githubRepository) {
  console.error("::error::GITHUB_REPOSITORY is required.");
  process.exit(1);
}

const [githubOwner, githubRepo] = githubRepository.split("/");
const tmpDir = path.join(process.env.RUNNER_TEMP || "/tmp", `gitee-release-${tag}`);

async function requestJson(url, options = {}) {
  const response = await fetch(url, options);
  const text = await response.text();
  let data = null;
  if (text) {
    try {
      data = JSON.parse(text);
    } catch {
      data = text;
    }
  }
  if (!response.ok) {
    const message = typeof data === "string" ? data : JSON.stringify(data);
    throw new Error(`${options.method || "GET"} ${url} failed: ${response.status} ${message}`);
  }
  return data;
}

function githubHeaders(extra = {}) {
  return {
    Accept: "application/vnd.github+json",
    Authorization: `Bearer ${githubToken}`,
    "X-GitHub-Api-Version": "2022-11-28",
    ...extra,
  };
}

function giteeParams(extra = {}) {
  return new URLSearchParams({ access_token: giteeToken, ...extra });
}

async function getGithubRelease() {
  return requestJson(`https://api.github.com/repos/${githubOwner}/${githubRepo}/releases/tags/${encodeURIComponent(tag)}`, {
    headers: githubHeaders(),
  });
}

async function ensureGiteeRelease(release) {
  const base = `https://gitee.com/api/v5/repos/${giteeOwner}/${giteeRepo}/releases`;
  const getUrl = `${base}/tags/${encodeURIComponent(tag)}?${giteeParams()}`;
  try {
    return await requestJson(getUrl);
  } catch (error) {
    if (!String(error.message).includes(" 404 ")) throw error;
  }

  const body = giteeParams({
    tag_name: tag,
    name: release.name || tag,
    body: release.body || "",
    prerelease: String(Boolean(release.prerelease)),
    target_commitish: release.target_commitish || "master",
  });
  return requestJson(base, {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body,
  });
}

async function updateGiteeRelease(releaseId, release) {
  const body = giteeParams({
    name: release.name || tag,
    body: release.body || "",
    prerelease: String(Boolean(release.prerelease)),
  });
  return requestJson(`https://gitee.com/api/v5/repos/${giteeOwner}/${giteeRepo}/releases/${releaseId}`, {
    method: "PATCH",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body,
  });
}

async function downloadGithubAsset(asset) {
  const response = await fetch(asset.url, {
    headers: githubHeaders({ Accept: "application/octet-stream" }),
  });
  if (!response.ok) {
    throw new Error(`download ${asset.name} failed: ${response.status} ${await response.text()}`);
  }
  const file = path.join(tmpDir, asset.name);
  await fs.writeFile(file, Buffer.from(await response.arrayBuffer()));
  return file;
}

async function deleteGiteeAsset(assetId) {
  await requestJson(`https://gitee.com/api/v5/repos/${giteeOwner}/${giteeRepo}/releases/attach_files/${assetId}?${giteeParams()}`, {
    method: "DELETE",
  });
}

async function uploadGiteeAsset(releaseId, file, name) {
  const form = new FormData();
  form.append("access_token", giteeToken);
  form.append("file", new Blob([await fs.readFile(file)]), name);
  const response = await fetch(`https://gitee.com/api/v5/repos/${giteeOwner}/${giteeRepo}/releases/${releaseId}/attach_files`, {
    method: "POST",
    body: form,
  });
  if (!response.ok) {
    throw new Error(`upload ${name} failed: ${response.status} ${await response.text()}`);
  }
}

async function rewriteLatestForGitee(file) {
  const manifest = JSON.parse(await fs.readFile(file, "utf8"));
  const base = `https://gitee.com/${giteeOwner}/${giteeRepo}/releases/download/${tag}`;
  for (const platform of Object.values(manifest.platforms || {})) {
    if (typeof platform.url === "string") {
      platform.url = `${base}/${encodeURIComponent(path.posix.basename(new URL(platform.url).pathname))}`;
    }
  }
  await fs.writeFile(file, `${JSON.stringify(manifest, null, 2)}\n`);
}

await fs.rm(tmpDir, { recursive: true, force: true });
await fs.mkdir(tmpDir, { recursive: true });

const githubRelease = await getGithubRelease();
const giteeRelease = await ensureGiteeRelease(githubRelease);
await updateGiteeRelease(giteeRelease.id, githubRelease);

const existingAssets = new Map((giteeRelease.attach_files || []).map((asset) => [asset.name, asset.id]));
const assets = githubRelease.assets || [];
console.log(`Syncing ${assets.length} asset(s) from GitHub release ${tag} to Gitee as ${giteeUsername}.`);

for (const asset of assets) {
  const file = await downloadGithubAsset(asset);
  if (asset.name === "latest.json" || asset.name === "latest-gitee.json") {
    await rewriteLatestForGitee(file);
  }
  if (existingAssets.has(asset.name)) {
    await deleteGiteeAsset(existingAssets.get(asset.name));
  }
  await uploadGiteeAsset(giteeRelease.id, file, asset.name);
  console.log(`Uploaded ${asset.name}`);
}
