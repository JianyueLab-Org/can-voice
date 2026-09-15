"""从 can-audio 的 Python 实现生成金文件，Rust 侧照着它断言。

两份实现在同一份电码上念出同一句话，才谈得上换掉旧的。跑法：

    cd crates/can-voice-atis/testdata && python3 gen_atis_golden.py > atis-golden.json

路径是相对本文件的，所以要在这个目录里跑。
"""
import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[4] / "can-audio" / "atis"))

import chinese                        # noqa: E402
import metar as metar_module          # noqa: E402
import template as template_module    # noqa: E402
import voicefix                       # noqa: E402

METARS = [
    "ZSPD 251300Z 09004MPS 9999 FEW030 SCT100 25/18 Q1013 NOSIG",
    "ZBAA 291000Z 30007MPS CAVOK 24/M08 Q1003 NOSIG",
    "ZGGG 010800Z 13003MPS 080V180 6000 -RA BKN010 OVC030 27/26 Q1005 NOSIG",
    "ZULS 020200Z 25006MPS 9999 SCT023 BKN046 12/M05 Q0995 NOSIG",
    "ZSSS 251200Z VRB01MPS 1200 R36L/1000 BR NSC 18/17 Q1020 NOSIG",
    "ZYTX 251300Z 09004MPS 9999 M03/M07 A2992 NOSIG",
    "KLAX 251300Z 26015G25KT 220V300 10SM FEW015 SCT200 25/18 A2992",
    "RJTT 251300Z 36012KT 9999 BKN020CB 28/24 Q1008 TEMPO 36020G35KT",
    "ZSPD 251300Z 09004MPS 3000 -SHRA R35L/1200 BKN010 25/18 Q1013",
    "ZHHH 251300Z AUTO 00000MPS 9999 RERA VV/// 25/18 Q1013 NOSIG",
    "METAR ZSAM 251300Z 09004MPS 9999 NSC 25/18 Q1013=",
    "ZPPP 251300Z 09004MPS 9999 XYZZY123 25/18 Q1013",
    "ZLXY 251300Z 34003MPS 4000 BR FEW006 SCT033 08/07 Q1024 BECMG TL0530 0800 FG",
    "ZWWW 251300Z 07008G14MPS 9999 -TSRA SCT030CB BKN040 22/16 Q1010 NOSIG",
    "",
]

FREE_TEXTS = [
    "ARR RWY 16L, 17R, DEP RWY 16R, 17L",
    "SIMUL PARL ILS APCHS TO RWY34L_R ARE INPR",
    "DEP FREQ 126.0, CTC TWR 118.2",
    "RWY 36Left",
    "TWY A CLSD DUE MAINT, EXP DELAY",
    "ILS CAT II APCH AVBL RWY 18L, RVR INOP",
    "跑道 35L 使用中",
    "BIRD ACTIVITY VC ARPT, ACFT ADVISED",
    "TRL 3600, TA 3000",
    "RNAV DEPS UNAVBL, ADVISE ATC ON FIRST CTC",
    "WIP BTN TWY B AND TWY C, GND FREQ 121.6",
    "",
]

TEMPLATES = [
    template_module.DEFAULT_TEMPLATE,
    "[FACILITY] ATIS [ATIS_LETTER] [OBS_TIME]. [WX]. [CLOSING]",
    "[WIND] [WIND:VOX]",
    "[FACILITY] INFORMATION [ATIS_LETTER]",
    "[WIND]. [RVR]. [PRESSURE]",
    "[NOPE] [WIND]",
    "[TEMP]/[DEW] [CLOUDS] [TREND] [RECENT_WX]",
    "TRL [TL]. @ils APCH IN USE. [ARPT_COND]",
]

CONTRACTIONS = {"ils": ("ILS", "I L S"), "vor": ("VOR", "")}

out = {
    "note": (
        "由 can-audio/atis 的 Python 实现生成（.temp/gen_atis_golden.py）。"
        "Rust 侧的 metar/voicefix/template 必须一字不差地念出同样的话。"
    ),
    "metars": [],
    "free_text": [
        {"input": t, "expand": voicefix.expand_free_text(t), "polish": voicefix.polish(t)}
        for t in FREE_TEXTS
    ],
    "renders": [],
    "chinese": [],
    "counts": [
        {"value": n, "spoken": chinese.spell_count(n)}
        for n in (0, 1, 5, 10, 11, 15, 20, 30, 100, 101, 110, 115, 120, 200,
                  305, 900, 960, 1000, 1005, 1050, 1500, 3000, 9999, -8, -25)
    ],
}

for raw in METARS:
    m = metar_module.Metar(raw)
    out["metars"].append({
        "raw": raw,
        "station": m.station,
        "valid": m.is_valid(),
        "cavok": m.cavok,
        "auto": m.auto,
        "elements": {
            name: {"text": getattr(m, name).text, "voice": getattr(m, name).voice}
            for name in ("observation_time", "wind", "visibility", "rvr",
                         "present_weather", "clouds", "temperature", "dew_point",
                         "pressure", "trend", "recent_weather")
        },
        "full_wx": {"text": m.full_wx().text, "voice": m.full_wx().voice},
    })

for raw in METARS:
    m = metar_module.Metar(raw)
    ctx = template_module.build_context(
        m, facility="ZSPD", letter="F",
        airport_conditions="ARR RWY 16L, 17R. ILS APCH IN USE",
        notams="TWY A CLSD DUE MAINT",
        transition_level="3600",
        facility_voice="Shanghai Pudong International Airport")
    for tpl in TEMPLATES:
        text, voice = template_module.render(tpl, ctx, CONTRACTIONS)
        out["renders"].append({
            "metar": raw, "template": tpl, "text": text, "voice": voice,
            "unknown": template_module.unknown_variables(tpl),
        })

CHINESE_SCRIPTS = [
    {"facility": "上海浦东", "letter": "A", "runway": "三五左", "extra": ""},
    {"facility": "北京首都", "letter": "J", "runway": "跑道独立平行离场，跑道 三六左 起始高度 六百米", "extra": "谨慎鸟情"},
    {"facility": "", "letter": "", "runway": "", "extra": ""},
    {"facility": "广州白云", "letter": "Z", "runway": "零两左", "extra": "机坪施工"},
]

for raw in METARS:
    m = metar_module.Metar(raw) if raw else None
    for sc in CHINESE_SCRIPTS:
        out["chinese"].append({
            "metar": raw,
            **sc,
            "script": chinese.render(m, facility=sc["facility"], letter=sc["letter"],
                                     runway=sc["runway"], extra=sc["extra"]),
        })
# metar 为 None 的那一路要单独钉住：没有报文时只念台名和字母。
for sc in CHINESE_SCRIPTS:
    out["chinese"].append({
        "metar": None, **sc,
        "script": chinese.render(None, facility=sc["facility"], letter=sc["letter"],
                                 runway=sc["runway"], extra=sc["extra"]),
    })

json.dump(out, sys.stdout, ensure_ascii=False, indent=1)
print()
