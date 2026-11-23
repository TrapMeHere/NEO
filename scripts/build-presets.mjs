// Turn the same npm packages iD uses into one preset list + categories.
//
// iD (modules/services/nsi.ts) loads id-tagging-schema presets, then calls
// name-suggestion-index's buildIDPresets and marks every result suggestion=true.
// When a wikidata brand/operator/network tag is present, the matching *:wikipedia
// key is removed so the two cannot drift apart (iD #9103).
//
// Categories come from preset_categories.json (same as iD's picker groups).

import { mkdirSync, readFileSync, unlinkSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { buildIDPresets } from "name-suggestion-index";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");

function pkgFile(name, rel) {
  return join(repoRoot, "node_modules", name, rel);
}

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

function translationNames(path, lang, section) {
  const root = readJson(path);
  const map = root?.[lang]?.presets?.[section] ?? {};
  const names = new Map();
  for (const [id, value] of Object.entries(map)) {
    if (value && typeof value.name === "string" && value.name.length > 0) {
      names.set(id, value.name);
    }
  }
  return names;
}

function tagPairs(tags) {
  if (!tags || typeof tags !== "object") return [];
  const pairs = [];
  for (const [key, value] of Object.entries(tags)) {
    if (typeof value === "string" && value !== "*") pairs.push([key, value]);
  }
  return pairs;
}

/** Matching tags keep `*` wildcards (iD `tags`); apply tags drop them. */
function matchPairs(tags) {
  if (!tags || typeof tags !== "object") return [];
  const pairs = [];
  for (const [key, value] of Object.entries(tags)) {
    if (typeof value === "string") pairs.push([key, value]);
  }
  return pairs;
}

function geometryOf(preset) {
  if (!Array.isArray(preset.geometry)) return [];
  return preset.geometry.filter((item) => typeof item === "string");
}

function removeKeys(preset) {
  const keys = [];
  const source = preset.removeTags;
  if (!source || typeof source !== "object") return keys;
  for (const [key, value] of Object.entries(source)) {
    if (typeof value === "string") keys.push(key);
  }
  return keys;
}

function applyWikipediaCleanup(preset) {
  const tags = preset.tags ?? {};
  if (tags["brand:wikidata"]) {
    preset.removeTags = { "brand:wikipedia": "*", ...(preset.removeTags || preset.addTags || preset.tags || {}) };
  }
  if (tags["operator:wikidata"]) {
    preset.removeTags = { "operator:wikipedia": "*", ...(preset.removeTags || preset.addTags || preset.tags || {}) };
  }
  if (tags["network:wikidata"]) {
    preset.removeTags = { "network:wikipedia": "*", ...(preset.removeTags || preset.addTags || preset.tags || {}) };
  }
}

const schemaPresets = readJson(pkgFile("@openstreetmap/id-tagging-schema", "dist/presets.json"));
const categoriesFile = readJson(
  pkgFile("@openstreetmap/id-tagging-schema", "dist/preset_categories.json"),
);
const enPresets = translationNames(
  pkgFile("@openstreetmap/id-tagging-schema", "dist/translations/en.json"),
  "en",
  "presets",
);
const zhPresets = translationNames(
  pkgFile("@openstreetmap/id-tagging-schema", "dist/translations/zh-CN.json"),
  "zh-CN",
  "presets",
);
const enCats = translationNames(
  pkgFile("@openstreetmap/id-tagging-schema", "dist/translations/en.json"),
  "en",
  "categories",
);
const zhCats = translationNames(
  pkgFile("@openstreetmap/id-tagging-schema", "dist/translations/zh-CN.json"),
  "zh-CN",
  "categories",
);
const nsi = readJson(pkgFile("name-suggestion-index", "dist/json/nsi.min.json"));
const dissolvedFile = readJson(pkgFile("name-suggestion-index", "dist/wikidata/dissolved.min.json"));

const built = buildIDPresets(nsi.nsi, {
  sourcePresets: schemaPresets,
  dissolved: dissolvedFile.dissolved ?? {},
}).presets;

for (const preset of Object.values(built)) {
  preset.suggestion = true;
  applyWikipediaCleanup(preset);
}

const rows = [];

for (const [id, preset] of Object.entries(schemaPresets)) {
  if (id.startsWith("@")) continue;
  const tags = tagPairs(preset.addTags ?? preset.tags);
  if (tags.length === 0) continue;
  const match = matchPairs(preset.tags ?? {});
  rows.push({
    id,
    zh: zhPresets.get(id) ?? preset.name ?? id,
    en: enPresets.get(id) ?? preset.name ?? id,
    tags,
    match: match.length > 0 ? match : tags,
    matchScore: typeof preset.matchScore === "number" ? preset.matchScore : 1,
    remove: [],
    geometry: geometryOf(preset),
    suggestion: false,
  });
}

for (const [id, preset] of Object.entries(built)) {
  if (preset.searchable === false) continue;
  const tags = tagPairs(preset.addTags ?? preset.tags);
  if (tags.length === 0) continue;
  const match = matchPairs(preset.tags ?? {});
  rows.push({
    id,
    zh: zhPresets.get(id) ?? preset.name ?? id,
    en: enPresets.get(id) ?? preset.name ?? id,
    tags,
    match: match.length > 0 ? match : tags,
    matchScore: typeof preset.matchScore === "number" ? preset.matchScore : 1,
    remove: removeKeys(preset),
    geometry: geometryOf(preset),
    suggestion: true,
  });
}

rows.sort((a, b) => a.id.localeCompare(b.id));

const knownIds = new Set(rows.map((row) => row.id));
const categories = [];
for (const [id, value] of Object.entries(categoriesFile)) {
  if (!value || typeof value !== "object") continue;
  const members = Array.isArray(value.members)
    ? value.members.filter((member) => typeof member === "string" && knownIds.has(member))
    : [];
  if (members.length === 0) continue;
  categories.push({
    id,
    zh: zhCats.get(id) ?? value.name ?? id.replace(/^category-/, ""),
    en: enCats.get(id) ?? value.name ?? id.replace(/^category-/, ""),
    members,
  });
}
categories.sort((a, b) => a.id.localeCompare(b.id));

const schemaPkg = readJson(pkgFile("@openstreetmap/id-tagging-schema", "package.json"));
const nsiPkg = readJson(pkgFile("name-suggestion-index", "package.json"));
const outDir = process.env.NEO_PRESET_DIR || join(repoRoot, "assets", "presets");
mkdirSync(outDir, { recursive: true });

const GEOM_BIT = { point: 1, vertex: 2, line: 4, area: 8, relation: 16 };

// 与 crates/neo-style/src/schema.rs 的读取器同一布局，小端。
// NEOP u16 version=1 u16 kind | u32 schema_idx u32 nsi_idx u32 string_count u32 blob_len u32 preset_count
// u32 offsets[string_count+1] | utf-8 blob
// 每条：id en zh(u32::MAX=与 en 相同) score_milli geom pad n_tags n_match n_remove pad
// 然后 (key,value) 对、match 对、remove 键，全是字符串表下标。
function encodePresets(kind, presetRows, schemaVersion, nsiVersion) {
  const strings = [];
  const index = new Map();
  const intern = (text) => {
    let id = index.get(text);
    if (id === undefined) {
      id = strings.length;
      index.set(text, id);
      strings.push(text);
    }
    return id;
  };
  const schemaIdx = intern(schemaVersion);
  const nsiIdx = intern(nsiVersion);
  const records = presetRows.map((row) => {
    let geom = 0;
    for (const name of row.geometry) geom |= GEOM_BIT[name] ?? 0;
    return {
      id: intern(row.id),
      en: intern(row.en),
      zh: row.zh === row.en ? 0xffffffff : intern(row.zh),
      score: Math.min(65535, Math.round(row.matchScore * 1000)),
      geom,
      tags: row.tags.map(([key, value]) => [intern(key), intern(value)]),
      match: row.match.map(([key, value]) => [intern(key), intern(value)]),
      remove: row.remove.map((key) => intern(key)),
    };
  });
  const encoded = strings.map((text) => Buffer.from(text, "utf8"));
  const blobLen = encoded.reduce((sum, buf) => sum + buf.length, 0);
  let body = 0;
  for (const record of records) {
    body += 24 + (record.tags.length + record.match.length) * 8 + record.remove.length * 4;
  }
  const total = 28 + (strings.length + 1) * 4 + blobLen + body;
  const buf = Buffer.allocUnsafe(total);
  let at = 0;
  buf.write("NEOP", at);
  at += 4;
  buf.writeUInt16LE(1, at);
  at += 2;
  buf.writeUInt16LE(kind, at);
  at += 2;
  buf.writeUInt32LE(schemaIdx, at);
  at += 4;
  buf.writeUInt32LE(nsiIdx, at);
  at += 4;
  buf.writeUInt32LE(strings.length, at);
  at += 4;
  buf.writeUInt32LE(blobLen, at);
  at += 4;
  buf.writeUInt32LE(records.length, at);
  at += 4;
  let cursor = 0;
  for (const part of encoded) {
    buf.writeUInt32LE(cursor, at);
    at += 4;
    cursor += part.length;
  }
  buf.writeUInt32LE(cursor, at);
  at += 4;
  for (const part of encoded) {
    part.copy(buf, at);
    at += part.length;
  }
  for (const record of records) {
    buf.writeUInt32LE(record.id, at);
    at += 4;
    buf.writeUInt32LE(record.en, at);
    at += 4;
    buf.writeUInt32LE(record.zh, at);
    at += 4;
    buf.writeUInt16LE(record.score, at);
    at += 2;
    buf.writeUInt8(record.geom, at);
    at += 1;
    buf.writeUInt8(0, at);
    at += 1;
    buf.writeUInt16LE(record.tags.length, at);
    at += 2;
    buf.writeUInt16LE(record.match.length, at);
    at += 2;
    buf.writeUInt16LE(record.remove.length, at);
    at += 2;
    buf.writeUInt16LE(0, at);
    at += 2;
    for (const [key, value] of record.tags) {
      buf.writeUInt32LE(key, at);
      at += 4;
      buf.writeUInt32LE(value, at);
      at += 4;
    }
    for (const [key, value] of record.match) {
      buf.writeUInt32LE(key, at);
      at += 4;
      buf.writeUInt32LE(value, at);
      at += 4;
    }
    for (const key of record.remove) {
      buf.writeUInt32LE(key, at);
      at += 4;
    }
  }
  if (at !== total) throw new Error(`preset encode size ${at} != ${total}`);
  return buf;
}

const schemaRows = rows.filter((row) => !row.suggestion);
const suggestionRows = rows.filter((row) => row.suggestion);
const schemaBytes = encodePresets(1, schemaRows, schemaPkg.version, nsiPkg.version);
const suggestionBytes = encodePresets(2, suggestionRows, schemaPkg.version, nsiPkg.version);
const schemaPath = join(outDir, "schema.neo");
const suggestionPath = join(outDir, "suggestions.neo");
writeFileSync(schemaPath, schemaBytes);
writeFileSync(suggestionPath, suggestionBytes);
try {
  unlinkSync(join(outDir, "presets.gen.json"));
} catch {
  // 旧 JSON 可能已经不在。
}
console.log(
  `presets schema=${schemaPkg.version} nsi=${nsiPkg.version} rows=${schemaRows.length} suggestions=${suggestionRows.length} categories=${categories.length} schema_bytes=${schemaBytes.length} suggestion_bytes=${suggestionBytes.length}`,
);
