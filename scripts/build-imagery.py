#!/usr/bin/env python3
"""Condense iD's data/imagery.json into a country-filtered imagery catalog.

iD uses polygons to decide where an imagery layer appears. This script reduces coverage to two forms:
worldwide, or a set of ISO 3166-1 alpha-2 country codes. Country borders come from Natural Earth 1:50m.

Output:
  assets/imagery/catalog.json
  assets/imagery/countries.json
"""

from __future__ import annotations

import json
import math
import re
import urllib.request
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CACHE = ROOT / "out" / "imagery-src"
OUT_DIR = ROOT / "assets" / "imagery"

IMAGERY_URL = "https://raw.githubusercontent.com/openstreetmap/iD/develop/data/imagery.json"
COUNTRIES_URL = (
    "https://raw.githubusercontent.com/nvkelso/natural-earth-vector/master/geojson/"
    "ne_50m_admin_0_countries.geojson"
)

# These are already built into the program (including the locally agreed Mapbox / Tianditu URLs), so skip them to avoid duplicating iD's catalog.
SKIP_IDS = {
    "bing",
    "esriworldimagery",
    "esriworldimageryclarity",
    "mapbox",
}


def fetch(url: str, name: str) -> Path:
    CACHE.mkdir(parents=True, exist_ok=True)
    path = CACHE / name
    if path.is_file() and path.stat().st_size > 0:
        return path
    print(f"download {url}")
    request = urllib.request.Request(url, headers={"User-Agent": "NEO/0.1 (imagery-index)"})
    with urllib.request.urlopen(request, timeout=120) as response:
        path.write_bytes(response.read())
    return path


def point_in_ring(lon: float, lat: float, ring: list[list[float]]) -> bool:
    inside = False
    count = len(ring)
    if count < 3:
        return False
    j = count - 1
    for i in range(count):
        xi, yi = ring[i][0], ring[i][1]
        xj, yj = ring[j][0], ring[j][1]
        if (yi > lat) != (yj > lat):
            x_cross = (xj - xi) * (lat - yi) / (yj - yi + 0.0) + xi
            if lon < x_cross:
                inside = not inside
        j = i
    return inside


def point_in_polygons(lon: float, lat: float, polygons: list[list[list[list[float]]]]) -> bool:
    for polygon in polygons:
        if not polygon:
            continue
        if not point_in_ring(lon, lat, polygon[0]):
            continue
        if any(point_in_ring(lon, lat, hole) for hole in polygon[1:]):
            continue
        return True
    return False


def bbox_of(polygons: list) -> tuple[float, float, float, float] | None:
    min_lon = min_lat = 999.0
    max_lon = max_lat = -999.0
    seen = False
    for polygon in polygons:
        for ring in polygon:
            for lon, lat in ring:
                seen = True
                min_lon = min(min_lon, lon)
                min_lat = min(min_lat, lat)
                max_lon = max(max_lon, lon)
                max_lat = max(max_lat, lat)
    if not seen:
        return None
    return (min_lon, min_lat, max_lon, max_lat)


def round_ring(ring: list) -> list[list[float]]:
    out = []
    for lon, lat, *_rest in ring:
        point = [round(float(lon), 2), round(float(lat), 2)]
        if not out or out[-1] != point:
            out.append(point)
    if len(out) >= 2 and out[0] == out[-1]:
        out.pop()
    return out if len(out) >= 3 else []


def geometry_polygons(geometry: dict) -> list[list[list[list[float]]]]:
    kind = geometry.get("type")
    coords = geometry.get("coordinates") or []
    if kind == "Polygon":
        rings = [round_ring(ring) for ring in coords]
        rings = [ring for ring in rings if ring]
        return [rings] if rings else []
    if kind == "MultiPolygon":
        polygons = []
        for polygon in coords:
            rings = [round_ring(ring) for ring in polygon]
            rings = [ring for ring in rings if ring]
            if rings:
                polygons.append(rings)
        return polygons
    return []


def iso_code(props: dict) -> str | None:
    code = str(props.get("ISO_A2") or "")
    if len(code) != 2 or not code.isalpha():
        code = str(props.get("ISO_A2_EH") or "")
    if len(code) != 2 or not code.isalpha():
        return None
    return code.upper()


def load_countries(path: Path) -> list[dict]:
    data = json.loads(path.read_text(encoding="utf-8"))
    countries = []
    for feature in data.get("features", []):
        props = feature.get("properties") or {}
        code = iso_code(props)
        geometry = feature.get("geometry") or {}
        polygons = geometry_polygons(geometry)
        if not code or not polygons:
            continue
        parts = []
        for polygon in polygons:
            box = polygon_bbox(polygon)
            parts.append({"b": [round(value, 2) for value in box], "r": polygon})
        countries.append(
            {
                "c": code,
                "en": str(props.get("NAME") or props.get("ADMIN") or code),
                "zh": str(props.get("NAME_ZH") or props.get("NAME_ZHT") or props.get("NAME") or code),
                "parts": parts,
            }
        )
    return countries


def polygon_bbox(polygon: list) -> tuple[float, float, float, float]:
    box = bbox_of([polygon])
    assert box is not None
    return box


def country_at(countries: list[dict], lon: float, lat: float) -> str | None:
    for country in countries:
        for part in country["parts"]:
            min_lon, min_lat, max_lon, max_lat = part["b"]
            if lon < min_lon or lon > max_lon or lat < min_lat or lat > max_lat:
                continue
            if point_in_polygons(lon, lat, [part["r"]]):
                return country["c"]
    return None


def imagery_polygons(source: dict) -> list[list[list[list[float]]]]:
    """iD's polygon is a list of outer rings, with no holes."""
    raw = source.get("polygon") or []
    polygons = []
    for ring in raw:
        rounded = round_ring(ring)
        if rounded:
            polygons.append([rounded])
    return polygons


def ring_lon_span(ring: list[list[float]]) -> float:
    lons = sorted({round(point[0], 4) for point in ring})
    if len(lons) < 2:
        return 0.0
    gaps = [b - a for a, b in zip(lons, lons[1:])]
    gaps.append(lons[0] + 360.0 - lons[-1])
    return 360.0 - max(gaps)


def ring_centroid(ring: list[list[float]]) -> tuple[float, float]:
    lons = [point[0] for point in ring]
    lats = [point[1] for point in ring]
    if max(lons) - min(lons) > 180.0:
        shifted = [lon + 360.0 if lon < 0.0 else lon for lon in lons]
        lon = sum(shifted) / len(shifted)
        if lon > 180.0:
            lon -= 360.0
    else:
        lon = sum(lons) / len(lons)
    return lon, sum(lats) / len(lats)


def is_worldwide(polygons: list) -> bool:
    if not polygons:
        return True
    for polygon in polygons:
        ring = polygon[0]
        lats = [point[1] for point in ring]
        if ring_lon_span(ring) >= 300.0 and max(lats) - min(lats) >= 90.0:
            return True
    return False


def nearest_country(countries: list[dict], lon: float, lat: float) -> str | None:
    best_code = None
    best_km = 2200.0
    for country in countries:
        for part in country["parts"]:
            min_lon, min_lat, max_lon, max_lat = part["b"]
            if lon < min_lon - 25 or lon > max_lon + 25 or lat < min_lat - 25 or lat > max_lat + 25:
                continue
            ring = part["r"][0]
            step = max(1, len(ring) // 24)
            for point in ring[::step]:
                scale = 111.0 * max(0.2, abs(math.cos(math.radians(lat))))
                kilometers = (((lon - point[0]) * scale) ** 2 + ((lat - point[1]) * 111.0) ** 2) ** 0.5
                if kilometers < best_km:
                    best_km = kilometers
                    best_code = country["c"]
    return best_code


def countries_for(source: dict, countries: list[dict]) -> list[str]:
    polygons = imagery_polygons(source)
    if is_worldwide(polygons):
        return []
    counts: Counter[str] = Counter()
    for polygon in polygons:
        ring = polygon[0]
        code = None
        lon, lat = ring_centroid(ring)
        if point_in_ring(lon, lat, ring):
            code = country_at(countries, lon, lat)
        if not code:
            lons = [point[0] for point in ring]
            lats = [point[1] for point in ring]
            min_lon, max_lon = min(lons), max(lons)
            min_lat, max_lat = min(lats), max(lats)
            found: Counter[str] = Counter()
            if max_lon - min_lon <= 180.0:
                for row in range(4):
                    for col in range(4):
                        sample_lon = min_lon + (max_lon - min_lon) * (col + 0.5) / 4.0
                        sample_lat = min_lat + (max_lat - min_lat) * (row + 0.5) / 4.0
                        if not point_in_ring(sample_lon, sample_lat, ring):
                            continue
                        hit = country_at(countries, sample_lon, sample_lat)
                        if hit:
                            found[hit] += 1
            if found:
                code = found.most_common(1)[0][0]
        if not code:
            code = nearest_country(countries, lon, lat)
        if code:
            counts[code] += 1
    if len(counts) >= 40:
        return []
    return [code for code, _hits in counts.most_common()]


def usable(source: dict) -> bool:
    if source.get("encrypted"):
        return False
    kind = source.get("type")
    if kind not in ("tms", "wms", "bing"):
        return False
    ident = str(source.get("id") or "")
    if ident.lower() in SKIP_IDS:
        return False
    url = str(source.get("template") or "")
    lowered = url.lower()
    if "mapbox.satellite" in lowered or "tianditu.gov.cn" in lowered or "s2cloudless" in lowered:
        return False
    if kind == "bing":
        return True
    if not url.startswith(("https://", "http://")):
        return False
    if kind == "wms":
        return "{bbox}" in lowered or "bbox=" in lowered
    return ("{x}" in lowered and ("{y}" in lowered or "{-y}" in lowered) and ("{z}" in lowered or "{zoom}" in lowered)) or "{quadkey}" in lowered or "{u}" in lowered


def max_zoom(source: dict) -> int:
    extent = source.get("zoomExtent") or []
    if isinstance(extent, list) and len(extent) >= 2:
        try:
            return max(0, min(22, int(extent[1])))
        except (TypeError, ValueError):
            pass
    return 22


def projection_of(source: dict) -> tuple[str, bool]:
    if source.get("type") != "wms":
        return "", False
    projection = str(source.get("projection") or "EPSG:3857")
    template = str(source.get("template") or "")
    yx = "1.3.0" in template and ("4326" in projection or "CRS:84" in projection)
    return projection, yx


def main() -> None:
    imagery_path = fetch(IMAGERY_URL, "id-imagery.json")
    countries_path = fetch(COUNTRIES_URL, "ne_50m_admin_0_countries.geojson")
    sources = json.loads(imagery_path.read_text(encoding="utf-8"))
    countries = load_countries(countries_path)
    kept = []
    skipped = 0
    for source in sources:
        if not usable(source):
            skipped += 1
            continue
        kind = source["type"]
        url = source.get("template") or ""
        # Never commit a plaintext Mapbox token; the program fills in an obfuscated token at runtime.
        if "access_token=pk." in url:
            url = re.sub(r"access_token=pk\.[^&\"'\\]+", "access_token={apikey}", url)
        if kind == "bing":
            url = "https://ecn.{switch:t0,t1,t2,t3}.tiles.virtualearth.net/tiles/a{quadkey}.jpeg?g=1"
            kind = "tms"
        crs, yx = projection_of(source)
        item = {
            "id": source["id"],
            "name": source.get("name") or source["id"],
            "type": kind,
            "url": url,
            "attribution": source.get("terms_text") or "",
            "max_zoom": max_zoom(source),
            "countries": countries_for(source, countries),
            "best": bool(source.get("best")),
        }
        if crs:
            item["crs"] = crs
        if yx:
            item["yx"] = True
        if source.get("overlay"):
            item["overlay"] = True
        kept.append(item)
    kept.sort(key=lambda item: (not item["best"], item["name"].lower(), item["id"]))
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    catalog = {"source": "iD data/imagery.json", "sources": kept}
    (OUT_DIR / "catalog.json").write_text(
        json.dumps(catalog, ensure_ascii=False, separators=(",", ":")),
        encoding="utf-8",
    )
    country_out = [
        {"c": item["c"], "en": item["en"], "zh": item["zh"], "p": item["parts"]} for item in countries
    ]
    (OUT_DIR / "countries.json").write_text(
        json.dumps(country_out, ensure_ascii=False, separators=(",", ":")),
        encoding="utf-8",
    )
    scope = Counter("world" if not item["countries"] else item["countries"][0] for item in kept)
    print(f"iD sources {len(sources)}, kept {len(kept)}, skipped {skipped}, countries {len(countries)}")
    print("top scopes", scope.most_common(12))
    print("catalog", (OUT_DIR / "catalog.json").stat().st_size, "countries", (OUT_DIR / "countries.json").stat().st_size)


if __name__ == "__main__":
    main()
