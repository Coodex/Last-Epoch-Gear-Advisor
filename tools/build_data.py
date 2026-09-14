"""Build the game-data files the Rust core embeds.

Sources (public, unauthenticated):
  * https://assets-ng.maxroll.gg/leplanner/game/data.json  - Maxroll's Last Epoch
    game data (the file LEBuildConverter's maxroll_names_data.json was cut from)
  * https://planners.maxroll.gg/profiles/le/<planner id>  - the guide's gear planner

Outputs (core/data/):
  affixes.json     id, name, display name, prefix/suffix, tiers (min/max roll), canRollOn,
                   modifier type (0 flat, 1 increased), stat key for "same stat" checks
  bases.json       item type id/name, sub type id/name, level requirement, implicit count
  uniques.json     id, name, base type, sub types
  planner_<id>.json  the planner document (gear per level bracket)

Run:  py -3.12 tools/build_data.py [guide-url-or-planner-id]
"""
from __future__ import annotations

import json
import re
import sys
import urllib.request
from pathlib import Path

DATA_URL = "https://assets-ng.maxroll.gg/leplanner/game/data.json"
PLANNER_API = "https://planners.maxroll.gg/profiles/le/{id}"
DEFAULT_GUIDE = "https://maxroll.gg/last-epoch/build-guides/paladin-leveling-guide"
OUT = Path(__file__).resolve().parents[1] / "core" / "data"
UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) LE-Gear-Advisor/0.1"


def fetch(url: str) -> bytes:
    req = urllib.request.Request(url, headers={"User-Agent": UA, "Accept": "*/*"})
    with urllib.request.urlopen(req, timeout=120) as resp:
        return resp.read()


def planner_id_from_guide(url: str) -> str:
    html = fetch(url).decode("utf-8", "replace")
    ids = re.findall(r"maxroll\.gg/last-epoch/planner/([A-Za-z0-9]+)", html)
    if not ids:
        raise SystemExit(f"no Maxroll planner embed found on {url}")
    return max(set(ids), key=ids.count)


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    arg = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_GUIDE
    planner_id = arg if re.fullmatch(r"[A-Za-z0-9]{6,12}", arg) else planner_id_from_guide(arg)
    print("planner id:", planner_id)

    raw = json.loads(fetch(PLANNER_API.format(id=planner_id)))
    raw["data"] = json.loads(raw["data"]) if isinstance(raw.get("data"), str) else raw.get("data")
    (OUT / f"planner_{planner_id}.json").write_text(json.dumps(raw, indent=1), encoding="utf-8")
    print("planner:", raw.get("name"), "profiles:", [p["name"] for p in raw["data"]["profiles"]])

    data = json.loads(fetch(DATA_URL))
    categories = data.get("affixDisplayCategories") or []
    affixes = []
    for a in data["affixes"]:
        affixes.append({
            "id": a["affixId"],
            "name": a["affixName"],
            "display_name": a.get("affixDisplayName") or a["affixName"],
            "kind": "suffix" if a.get("type") == 1 else "prefix",
            "tiers": [[t["minRoll"], t["maxRoll"]] for t in a.get("tiers") or []],
            "can_roll_on": a.get("canRollOn") or [],
            "modifier_type": a.get("modifierType"),
            "property": a.get("property"),
            "tags": a.get("tags", 0),
            "special_tag": a.get("specialTag", 0),
            "level": a.get("levelRequirement", 0),
            "category": categories[a["displayCategory"]] if 0 <= a.get("displayCategory", -1) < len(categories) else "",
        })
    (OUT / "affixes.json").write_text(json.dumps(affixes, separators=(",", ":")), encoding="utf-8")

    props = data.get("properties") or []
    bases = []
    for t in data["itemTypes"]:
        if t["baseTypeID"] >= 100:
            continue
        bases.append({
            "type_id": t["baseTypeID"],
            "type_name": t.get("displayName") or t.get("BaseTypeName") or "",
            "is_weapon": bool(t.get("isWeapon")),
            "max_affixes": t.get("maximumAffixes", 4),
            "sub_types": [{
                "id": s["subTypeID"],
                "name": s.get("displayName") or s.get("name") or "",
                "level": s.get("levelRequirement", 0),
                "implicits": [
                    (props[i["property"]].get("propertyName") if 0 <= i.get("property", -1) < len(props) else "")
                    for i in s.get("implicits") or []
                ],
            } for s in t.get("subItems") or []],
        })
    (OUT / "bases.json").write_text(json.dumps(bases, separators=(",", ":")), encoding="utf-8")

    uniques = [{"id": u["uniqueID"], "name": u.get("displayName") or u["name"], "base_type": u.get("baseType", 0),
                "sub_types": u.get("subTypes") or [], "level": u.get("levelRequirement", 0)}
               for u in data["uniques"]]
    (OUT / "uniques.json").write_text(json.dumps(uniques, separators=(",", ":")), encoding="utf-8")

    meta = {"data_url": DATA_URL, "planner_id": planner_id, "affixes": len(affixes),
            "bases": sum(len(b["sub_types"]) for b in bases), "uniques": len(uniques)}
    (OUT / "meta.json").write_text(json.dumps(meta, indent=1), encoding="utf-8")
    print(meta)


if __name__ == "__main__":
    main()
