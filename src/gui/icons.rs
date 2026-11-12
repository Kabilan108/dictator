//! Line icons on a 24 point grid, drawn in the current text colour.

use rune::Icon;

macro_rules! line_icon {
    ($static:ident, $name:literal, [$($d:literal),+ $(,)?]) => {
        pub(super) static $static: Icon = Icon {
            name: $name,
            view: 24.0,
            body: concat!($(
                "<path d=\"", $d,
                "\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.75\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>"
            ),+),
        };
    };
}

macro_rules! solid_icon {
    ($static:ident, $name:literal, [$($d:literal),+ $(,)?]) => {
        pub(super) static $static: Icon = Icon {
            name: $name,
            view: 24.0,
            body: concat!($("<path d=\"", $d, "\" fill=\"currentColor\"/>"),+),
        };
    };
}

line_icon!(
    MIC,
    "dictator-mic",
    [
        "M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z",
        "M19 10v2a7 7 0 0 1-14 0v-2",
        "M12 19v3",
    ]
);
line_icon!(
    HISTORY,
    "dictator-history",
    [
        "M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8",
        "M3 3v5h5",
        "M12 7v5l4 2",
    ]
);
line_icon!(
    CHART,
    "dictator-chart",
    ["M3 3v18h18", "M18 17V9", "M13 17V5", "M8 17v-3"]
);
line_icon!(
    SLIDERS,
    "dictator-sliders",
    [
        "M20 7h-9",
        "M14 17H5",
        "M14 17a3 3 0 1 0 6 0a3 3 0 1 0-6 0",
        "M4 7a3 3 0 1 0 6 0a3 3 0 1 0-6 0",
    ]
);
line_icon!(
    COPY,
    "dictator-copy",
    [
        "M10 8h10a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H10a2 2 0 0 1-2-2V10a2 2 0 0 1 2-2z",
        "M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2",
    ]
);
line_icon!(
    TRASH,
    "dictator-trash",
    [
        "M3 6h18",
        "M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6",
        "M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2",
        "M10 11v6",
        "M14 11v6",
    ]
);
line_icon!(CLOSE, "dictator-close", ["M18 6 6 18", "m6 6 12 12"]);
line_icon!(CHEVRON_UP, "dictator-chevron-up", ["m18 15-6-6-6 6"]);
line_icon!(CHEVRON_DOWN, "dictator-chevron-down", ["m6 9 6 6 6-6"]);
line_icon!(CHEVRON_LEFT, "dictator-chevron-left", ["m15 18-6-6 6-6"]);
line_icon!(CHEVRON_RIGHT, "dictator-chevron-right", ["m9 18 6-6-6-6"]);
line_icon!(
    SEARCH,
    "dictator-search",
    ["m21 21-4.3-4.3", "M3 11a8 8 0 1 0 16 0a8 8 0 1 0-16 0"]
);
line_icon!(
    BLUETOOTH,
    "dictator-bluetooth",
    ["m7 7 10 10-5 5V2l5 5L7 17"]
);
solid_icon!(
    PLAY,
    "dictator-play",
    ["M7 4.5v15a1 1 0 0 0 1.5.86l12.5-7.5a1 1 0 0 0 0-1.72L8.5 3.64A1 1 0 0 0 7 4.5z"]
);
solid_icon!(
    PAUSE,
    "dictator-pause",
    [
        "M7 4h3a1 1 0 0 1 1 1v14a1 1 0 0 1-1 1H7a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1z",
        "M14 4h3a1 1 0 0 1 1 1v14a1 1 0 0 1-1 1h-3a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1z",
    ]
);
