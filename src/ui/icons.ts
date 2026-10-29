// Inline SVG icons drawn after the SF Symbols the Mac app uses (24×24 grid, round joins, currentColor).
// One file on purpose: icons are tiny strings, built into DOM nodes on demand.

const S = (d: string) => `<path d="${d}" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/>`;
const F = (d: string) => `<path d="${d}" fill="currentColor"/>`;

const PATHS: Record<string, string> = {
  // window / layout
  sidebar: S("M4 5.5h16a1.5 1.5 0 0 1 1.5 1.5v10a1.5 1.5 0 0 1-1.5 1.5H4A1.5 1.5 0 0 1 2.5 17V7A1.5 1.5 0 0 1 4 5.5Z") + S("M9 5.5v13M5 9h1.5M5 11.5h1.5"),
  // build
  play: F("M8 5.2v13.6c0 .8.9 1.3 1.6.9l10.6-6.8c.6-.4.6-1.3 0-1.7L9.6 4.3C8.9 3.9 8 4.4 8 5.2Z"),
  stop: F("M7 6h10a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1H7a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1Z"),
  gears:
    S("M9 4.5l.6 1.6 1.7.4 1.2-1.2 1.4 1.4-1.2 1.2.4 1.7 1.6.6v2l-1.6.6-.4 1.7 1.2 1.2-1.4 1.4-1.2-1.2-1.7.4L9 16.5H7l-.6-1.6-1.7-.4-1.2 1.2-1.4-1.4 1.2-1.2-.4-1.7L1.3 10.8v-2l1.6-.6.4-1.7-1.2-1.2 1.4-1.4 1.2 1.2 1.7-.4L7 4.5Z") +
    `<circle cx="8" cy="9.8" r="2.1" fill="none" stroke="currentColor" stroke-width="1.6"/>` +
    S("M17 13.5l.5 1.2 1.3.2.8-.9 1 1-.9.8.2 1.3 1.2.5v1.4l-1.2.5-.2 1.3.9.8-1 1-.8-.9-1.3.2-.5 1.2h-1.4l-.5-1.2-1.3-.2-.8.9-1-1 .9-.8-.2-1.3-1.2-.5v-1.4l1.2-.5.2-1.3-.9-.8 1-1 .8.9 1.3-.2.5-1.2Z"),
  bolt: F("M13.4 2.5 5.2 13.2c-.4.5 0 1.1.6 1.1h5.3l-1.5 7c-.2.8.8 1.2 1.3.6l8-10.6c.4-.5 0-1.2-.6-1.2h-5.3l1.6-7c.2-.7-.8-1.2-1.2-.6Z"),
  boltOutline: S("M13.4 2.5 5.2 13.2c-.4.5 0 1.1.6 1.1h5.3l-1.5 7c-.2.8.8 1.2 1.3.6l8-10.6c.4-.5 0-1.2-.6-1.2h-5.3l1.6-7c.2-.7-.8-1.2-1.2-.6Z"),
  docRich: S("M6.5 2.8h7.8l4.2 4.2v12.5a1.7 1.7 0 0 1-1.7 1.7H6.5a1.7 1.7 0 0 1-1.7-1.7V4.5a1.7 1.7 0 0 1 1.7-1.7Z") + S("M14 3v4.3h4.3") + F("M7.8 11.2h8.4v6.3H7.8z"),
  detach: S("M10 4.5H6.5A2 2 0 0 0 4.5 6.5v11a2 2 0 0 0 2 2h11a2 2 0 0 0 2-2V14") + S("M13.5 4.5h6v6M19.3 4.7l-8.3 8.3"),
  attach: S("M14 4.5v5.5h5.5M19.5 4.5 14 10M10 19.5V14H4.5M4.5 19.5 10 14"),
  warning: S("M10.3 4.2 2.9 17.4c-.8 1.3.2 2.9 1.7 2.9h14.8c1.5 0 2.5-1.6 1.7-2.9L13.7 4.2c-.8-1.3-2.6-1.3-3.4 0Z") + S("M12 9v4.6") + F("M12 15.6a1.2 1.2 0 1 1 0 2.4 1.2 1.2 0 0 1 0-2.4Z"),
  warningFill:
    `<path fill-rule="evenodd" fill="currentColor" d="M10.3 4.2 2.9 17.4c-.8 1.3.2 2.9 1.7 2.9h14.8c1.5 0 2.5-1.6 1.7-2.9L13.7 4.2c-.8-1.3-2.6-1.3-3.4 0ZM11 9a1 1 0 0 1 2 0l-.2 4.8a.8.8 0 0 1-1.6 0L11 9Zm1 6.4a1.2 1.2 0 1 1 0 2.4 1.2 1.2 0 0 1 0-2.4Z"/>`,
  errorFill:
    `<path fill-rule="evenodd" fill="currentColor" d="M8.4 2.5h7.2l5 5v9l-5 5H8.4l-5-5v-9l5-5Zm.9 5.4a.9.9 0 0 0-1.3 1.3L10.7 12 8 14.8a.9.9 0 1 0 1.3 1.3l2.7-2.8 2.8 2.8a.9.9 0 0 0 1.3-1.3L13.3 12l2.8-2.8a.9.9 0 1 0-1.3-1.3L12 10.7 9.3 7.9Z"/>`,
  checkFill: `<path fill-rule="evenodd" fill="currentColor" d="M12 2.5a9.5 9.5 0 1 1 0 19 9.5 9.5 0 0 1 0-19Zm4.2 6.2a.9.9 0 0 0-1.3.1l-4 5.1-1.9-1.9a.9.9 0 0 0-1.3 1.3l2.6 2.6c.4.4 1 .3 1.3-.1l4.7-5.9a.9.9 0 0 0-.1-1.2Z"/>`,
  checkCircle: `<circle cx="12" cy="12" r="9" fill="none" stroke="currentColor" stroke-width="1.7"/>` + S("M8 12.3l2.7 2.7 5.3-6"),
  circleDashed: `<circle cx="12" cy="12" r="8.5" fill="none" stroke="currentColor" stroke-width="1.7" stroke-dasharray="3.2 2.6"/>`,
  info: `<circle cx="12" cy="12" r="9" fill="none" stroke="currentColor" stroke-width="1.7"/>` + S("M12 11v5.5") + F("M12 6.8a1.2 1.2 0 1 1 0 2.4 1.2 1.2 0 0 1 0-2.4Z"),
  badbox: S("M4 6h11M4 10.5h16M4 15h9") + S("M16 16.5h5"),
  // files
  folder: S("M3 7.2V17.5A1.8 1.8 0 0 0 4.8 19.3h14.4A1.8 1.8 0 0 0 21 17.5V9.3a1.8 1.8 0 0 0-1.8-1.8h-7.4L10 5.2H4.8A1.8 1.8 0 0 0 3 7Z") + S("M3 9.5h18"),
  folderFill: F("M2.5 7a2 2 0 0 1 2-2h5.4c.5 0 .9.2 1.3.5L13 7.3h6.6a2 2 0 0 1 2 2v8.4a2 2 0 0 1-2 2H4.5a2 2 0 0 1-2-2V7Z"),
  doc: S("M6.5 2.8h7.8l4.2 4.2v12.5a1.7 1.7 0 0 1-1.7 1.7H6.5a1.7 1.7 0 0 1-1.7-1.7V4.5a1.7 1.7 0 0 1 1.7-1.7Z") + S("M14 3v4.3h4.3"),
  docText: S("M6.5 2.8h7.8l4.2 4.2v12.5a1.7 1.7 0 0 1-1.7 1.7H6.5a1.7 1.7 0 0 1-1.7-1.7V4.5a1.7 1.7 0 0 1 1.7-1.7Z") + S("M14 3v4.3h4.3M8.3 11.5h7.4M8.3 14.5h7.4M8.3 17.5h4.6"),
  docPlus: S("M12 21.2H6.5a1.7 1.7 0 0 1-1.7-1.7V4.5a1.7 1.7 0 0 1 1.7-1.7h7.8l4.2 4.2V12") + S("M14 3v4.3h4.3M18 15v6M15 18h6"),
  books: S("M4 4.5h3v15H4zM8.5 4.5h3v15h-3z") + S("M13.3 5.6l2.9-.8 3.9 14.6-2.9.8z"),
  puzzle: S("M5 8h3.2a1.8 1.8 0 1 1 3.6 0H15v3.2a1.8 1.8 0 1 1 0 3.6V18H11.8a1.8 1.8 0 1 0-3.6 0H5v-3.2a1.8 1.8 0 1 0 0-3.6Z"),
  image: S("M4.5 5h15a1.5 1.5 0 0 1 1.5 1.5v11a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 17.5v-11A1.5 1.5 0 0 1 4.5 5Z") + S("M3.5 16l4.6-4.6 3.4 3.4 2.6-2.6 6.4 6.4") + `<circle cx="15.5" cy="9" r="1.5" fill="currentColor"/>`,
  star: S("M12 3.3l2.6 5.4 5.9.8-4.3 4.1 1 5.9L12 16.7l-5.2 2.8 1-5.9-4.3-4.1 5.9-.8Z"),
  starFill: F("M12 2.6c.3 0 .6.2.7.5l2.4 5 5.4.8c.7.1 1 .9.5 1.4l-3.9 3.8.9 5.4c.1.7-.6 1.2-1.2.9L12 17.8l-4.8 2.6c-.6.3-1.3-.2-1.2-.9l.9-5.4-3.9-3.8c-.5-.5-.2-1.3.5-1.4l5.4-.8 2.4-5c.1-.3.4-.5.7-.5Z"),
  chevronDown: S("M6.5 9.5 12 15l5.5-5.5"),
  chevronRight: S("M9.5 6.5 15 12l-5.5 5.5"),
  chevronUp: S("M6.5 14.5 12 9l5.5 5.5"),
  plus: S("M12 5v14M5 12h14"),
  minus: S("M5 12h14"),
  xmark: S("M6.5 6.5l11 11M17.5 6.5l-11 11"),
  xmarkCircleFill: `<path fill-rule="evenodd" fill="currentColor" d="M12 2.5a9.5 9.5 0 1 1 0 19 9.5 9.5 0 0 1 0-19ZM9.3 8a.9.9 0 0 0-1.3 1.3l2.7 2.7L8 14.7a.9.9 0 1 0 1.3 1.3l2.7-2.7 2.7 2.7a.9.9 0 0 0 1.3-1.3L13.3 12 16 9.3A.9.9 0 0 0 14.7 8L12 10.7 9.3 8Z"/>`,
  tag: S("M3.5 4.8v6.1c0 .5.2.9.5 1.2l8.4 8.4c.6.6 1.6.6 2.2 0l5.8-5.8c.6-.6.6-1.6 0-2.2L12 4.1a1.7 1.7 0 0 0-1.2-.5H4.7c-.7 0-1.2.5-1.2 1.2Z") + `<circle cx="7.7" cy="7.8" r="1.5" fill="currentColor"/>`,
  magnifier: `<circle cx="10.5" cy="10.5" r="6" fill="none" stroke="currentColor" stroke-width="1.8"/>` + S("M15 15l5 5"),
  command: S("M9 9V6.5A2.5 2.5 0 1 0 6.5 9H9Zm0 0h6m-6 0v6m6-6V6.5A2.5 2.5 0 1 1 17.5 9H15Zm0 0v6m0 0H9m6 0v2.5a2.5 2.5 0 1 0 2.5-2.5H15ZM9 15v2.5A2.5 2.5 0 1 1 6.5 15H9Z"),
  keyboard: S("M3.5 6.5h17a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1h-17a1 1 0 0 1-1-1v-9a1 1 0 0 1 1-1Z") + S("M6 10h.5M9 10h.5M12 10h.5M15 10h.5M18 10h.5M6 13h.5M18 13h.5M8.5 14h7"),
  gear:
    S("M10.3 3.2h3.4l.5 2.3 1.6.9 2.2-.8 1.7 2.9-1.8 1.6v1.8l1.8 1.6-1.7 2.9-2.2-.8-1.6.9-.5 2.3h-3.4l-.5-2.3-1.6-.9-2.2.8-1.7-2.9 1.8-1.6v-1.8L4.3 8.5 6 5.6l2.2.8 1.6-.9Z") +
    `<circle cx="12" cy="12" r="2.8" fill="none" stroke="currentColor" stroke-width="1.7"/>`,
  cursorText: S("M9 4.5h6M9 19.5h6M12 4.5v15") + S("M4 9l-1.5 3L4 15M20 9l1.5 3L20 15"),
  function: S("M15.5 4.3c-2.3-.6-3.7.6-4.2 3.2L9.4 17.3c-.5 2.4-1.9 3.4-4 2.7M7.5 10h8") + S("M14.5 13.5l4.5 5M19 13.5l-4.5 5"),
  listIndent: S("M4 6h16M8 10.5h12M8 15h12M4 19.5h16") + F("M3.5 10.2l2.8 2.3-2.8 2.3z"),
  bookClosed: S("M5.5 4.8A1.8 1.8 0 0 1 7.3 3H19v15H7.3a1.8 1.8 0 0 0-1.8 1.8ZM5.5 19.8A1.8 1.8 0 0 0 7.3 21.5H19V18") + S("M9 3v8l2.2-1.5L13.4 11V3"),
  playRect: S("M4 4.5h16a1.5 1.5 0 0 1 1.5 1.5v12a1.5 1.5 0 0 1-1.5 1.5H4A1.5 1.5 0 0 1 2.5 18V6A1.5 1.5 0 0 1 4 4.5Z") + F("M10 8.8v6.4c0 .5.6.8 1 .5l4.8-3.2c.4-.3.4-.8 0-1.1L11 8.3c-.4-.3-1 0-1 .5Z"),
  checklist: S("M3.5 6.5l1.6 1.6 2.6-3M3.5 12.5l1.6 1.6 2.6-3M3.5 18.5l1.6 1.6 2.6-3M11 7h9.5M11 13h9.5M11 19h9.5"),
  shippingbox: S("M12 2.8l8.3 4.3v9.8L12 21.2l-8.3-4.3V7.1Z") + S("M3.9 7.2 12 11.4l8.1-4.2M12 11.4v9.6M7.9 5l8.2 4.3"),
  sync: S("M19.5 9A7.8 7.8 0 0 0 5.5 7.5M4.5 15a7.8 7.8 0 0 0 14 1.5") + S("M5.3 3.8v3.9h3.9M18.7 20.2v-3.9h-3.9"),
  trash: S("M4.5 6.5h15M9.5 6.5V4.8c0-.7.6-1.3 1.3-1.3h2.4c.7 0 1.3.6 1.3 1.3v1.7M6.5 6.5l.9 12.6c.1 1 .9 1.7 1.8 1.7h5.6c1 0 1.7-.7 1.8-1.7l.9-12.6") + S("M10.3 10.5v6.5M13.7 10.5v6.5"),
  house: S("M3.5 11.5 12 4l8.5 7.5M6 9.5v9.3c0 .6.4 1 1 1h3.5v-5h3v5H17c.6 0 1-.4 1-1V9.5"),
  clock: `<circle cx="12" cy="12" r="9" fill="none" stroke="currentColor" stroke-width="1.7"/>` + S("M12 7v5.2l3.3 2"),
  filter: `<circle cx="12" cy="12" r="9" fill="none" stroke="currentColor" stroke-width="1.7"/>` + S("M7.5 9.5h9M9 12.5h6M10.5 15.5h3"),
  textViewfinder: S("M4 8V5.5A1.5 1.5 0 0 1 5.5 4H8M16 4h2.5A1.5 1.5 0 0 1 20 5.5V8M20 16v2.5a1.5 1.5 0 0 1-1.5 1.5H16M8 20H5.5A1.5 1.5 0 0 1 4 18.5V16M8 9.5h8M8 12.5h8M8 15.5h5"),
  percent: S("M18 6 6 18") + `<circle cx="7.5" cy="7.5" r="2.3" fill="none" stroke="currentColor" stroke-width="1.7"/><circle cx="16.5" cy="16.5" r="2.3" fill="none" stroke="currentColor" stroke-width="1.7"/>`,
  braces: S("M8.5 4H8a2 2 0 0 0-2 2v3.5c0 1.2-.8 2.5-2 2.5 1.2 0 2 1.3 2 2.5V18a2 2 0 0 0 2 2h.5M15.5 4h.5a2 2 0 0 1 2 2v3.5c0 1.2.8 2.5 2 2.5-1.2 0-2 1.3-2 2.5V18a2 2 0 0 1-2 2h-.5"),
  number: S("M9.5 4 7.5 20M16.5 4l-2 16M5 9h15M4 15h15"),
  dollar: S("M12 3v18M16.5 7.5c-.6-1.4-2.4-2.3-4.5-2.3-2.6 0-4.3 1.4-4.3 3.3 0 4.6 9.2 2.4 9.2 7 0 2-1.9 3.4-4.6 3.4-2.3 0-4.1-1-4.8-2.6"),
  divide: S("M5 12h14") + `<circle cx="12" cy="6.5" r="1.6" fill="currentColor"/><circle cx="12" cy="17.5" r="1.6" fill="currentColor"/>`,
  superscript: S("M3.5 8l8 10M11.5 8l-8 10") + S("M15.5 6.2c.3-1.1 1.2-1.7 2.3-1.7 1.3 0 2.2.8 2.2 1.9 0 1.9-3.2 2.7-4.4 4.6H20"),
  subscript: S("M3.5 6l8 10M11.5 6l-8 10") + S("M15.5 15.2c.3-1.1 1.2-1.7 2.3-1.7 1.3 0 2.2.8 2.2 1.9 0 1.9-3.2 2.7-4.4 4.6H20"),
  bold: S("M7 4.5h6a3.6 3.6 0 0 1 0 7.2H7ZM7 11.7h7a3.9 3.9 0 0 1 0 7.8H7Z"),
  italic: S("M10 4.5h8M6 19.5h8M14.5 4.5l-5 15"),
  textformat: S("M3 19 8.5 5h1L15 19M5 14.5h8") + S("M17 11.5c.4-.9 1.2-1.4 2.3-1.4 1.5 0 2.2.8 2.2 2.1V19M21.5 14.5c-3.3 0-4.8.8-4.8 2.4 0 1.2.9 2.1 2.3 2.1 1.5 0 2.5-1 2.5-2.5"),
  arrowRightLine: S("M3.5 12h12M11 7.5l4.5 4.5-4.5 4.5M20 5v14"),
  indent: S("M4 5h16M11 10h9M11 14h9M4 19h16") + S("M4 9.5l3 2.5-3 2.5"),
  outdent: S("M4 5h16M11 10h9M11 14h9M4 19h16") + S("M7 9.5 4 12l3 2.5"),
  arrowLeftSquare: S("M5 3.5h14A1.5 1.5 0 0 1 20.5 5v14a1.5 1.5 0 0 1-1.5 1.5H5A1.5 1.5 0 0 1 3.5 19V5A1.5 1.5 0 0 1 5 3.5Z") + S("M16 12H8.5M11.5 8.5 8 12l3.5 3.5"),
  arrowRightSquare: S("M5 3.5h14A1.5 1.5 0 0 1 20.5 5v14a1.5 1.5 0 0 1-1.5 1.5H5A1.5 1.5 0 0 1 3.5 19V5A1.5 1.5 0 0 1 5 3.5Z") + S("M8 12h7.5M12.5 8.5 16 12l-3.5 3.5"),
  square: S("M5 3.5h14A1.5 1.5 0 0 1 20.5 5v14a1.5 1.5 0 0 1-1.5 1.5H5A1.5 1.5 0 0 1 3.5 19V5A1.5 1.5 0 0 1 5 3.5Z"),
  chevronDownCircle: `<circle cx="12" cy="12" r="9" fill="none" stroke="currentColor" stroke-width="1.7"/>` + S("M8 10.5l4 4 4-4"),
  chevronUpCircle: `<circle cx="12" cy="12" r="9" fill="none" stroke="currentColor" stroke-width="1.7"/>` + S("M8 13.5l4-4 4 4"),
  save: S("M12 3.5v11M7.5 10l4.5 4.5 4.5-4.5M4.5 15v3.5a2 2 0 0 0 2 2h11a2 2 0 0 0 2-2V15"),
  chart: S("M4 20h16M7 16.5V11M11 16.5V6M15 16.5V9M19 16.5v-3"),
  ellipsis: `<circle cx="6" cy="12" r="1.6" fill="currentColor"/><circle cx="12" cy="12" r="1.6" fill="currentColor"/><circle cx="18" cy="12" r="1.6" fill="currentColor"/>`,
  copy: S("M8.5 8.5h10a1.5 1.5 0 0 1 1.5 1.5v9a1.5 1.5 0 0 1-1.5 1.5h-10A1.5 1.5 0 0 1 7 19v-9a1.5 1.5 0 0 1 1.5-1.5Z") + S("M16.5 8.5V5A1.5 1.5 0 0 0 15 3.5H5.5A1.5 1.5 0 0 0 4 5v9a1.5 1.5 0 0 0 1.5 1.5H7"),
  zoomFit: S("M4 9V5.5A1.5 1.5 0 0 1 5.5 4H9M15 4h3.5A1.5 1.5 0 0 1 20 5.5V9M20 15v3.5a1.5 1.5 0 0 1-1.5 1.5H15M9 20H5.5A1.5 1.5 0 0 1 4 18.5V15"),
};

/** Names of every icon (for type-checked usage). */
export type IconName = keyof typeof PATHS;

/** An inline SVG element for `name`, `size` CSS pixels square. */
export function icon(name: IconName | string, size = 16, cls = ""): SVGSVGElement {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("width", String(size));
  svg.setAttribute("height", String(size));
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("class", "icon" + (cls ? " " + cls : ""));
  svg.innerHTML = PATHS[name] ?? PATHS.doc ?? "";
  return svg;
}

/** Icon for a file in the tree / tab strip, after the Mac app's choices. */
export function fileIconName(path: string): string {
  const ext = path.slice(path.lastIndexOf(".") + 1).toLowerCase();
  switch (ext) {
    case "bib": return "books";
    case "sty": case "cls": return "puzzle";
    case "tex": case "ltx": return "docText";
    case "png": case "jpg": case "jpeg": case "pdf": case "eps": case "svg": return "image";
    default: return "doc";
  }
}

/** SF Symbol name (as used by the Rust templates / Mac catalog) → our icon. */
export function symbolIcon(symbol: string): string {
  const map: Record<string, string> = {
    "doc.text": "docText", "list.bullet.indent": "listIndent", "book.closed": "bookClosed", "play.rectangle": "playRect",
    function: "function", checklist: "checklist", "plus.rectangle.on.folder": "plus", "doc.badge.plus": "docPlus",
    folder: "folder", doc: "doc", xmark: "xmark", "xmark.rectangle": "xmark", "square.and.arrow.down": "save",
    "square.and.arrow.down.on.square": "save", "folder.badge.questionmark": "folder", tag: "tag", magnifyingglass: "magnifier",
    "text.magnifyingglass": "magnifier", "arrow.right.to.line": "arrowRightLine", percent: "percent", "increase.indent": "indent",
    "decrease.indent": "outdent", "text.badge.plus": "plus", dollarsign: "dollar", divide: "divide",
    "textformat.superscript": "superscript", "textformat.subscript": "subscript", bold: "bold", italic: "italic",
    textformat: "textformat", curlybraces: "braces", number: "number", "doc.richtext": "docRich",
    "arrow.up.forward.app": "detach", "exclamationmark.triangle": "warning", "doc.text.magnifyingglass": "magnifier",
    command: "command", "chart.bar.doc.horizontal": "chart", "arrow.right.square": "arrowRightSquare",
    "arrow.left.square": "arrowLeftSquare", "play.fill": "play", "stop.fill": "stop", bolt: "boltOutline",
    "gearshape.2": "gears", star: "star", "star.slash": "star", trash: "trash", "text.viewfinder": "textViewfinder",
    "doc.viewfinder": "doc", "chevron.down.circle": "chevronDownCircle", "chevron.up.circle": "chevronUpCircle",
    house: "house", keyboard: "keyboard", gearshape: "gear",
  };
  return map[symbol] ?? (/^\d\.square$/.test(symbol) ? "square" : "doc");
}

/** The app icon: a white ∂ on a blue squircle (vector, so it stays crisp at any scale). */
export function appIcon(size = 104): SVGSVGElement {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 100 100");
  svg.setAttribute("width", String(size));
  svg.setAttribute("height", String(size));
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("class", "app-icon");
  svg.innerHTML =
    `<defs><linearGradient id="gtx-g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#5b8df0"/><stop offset="1" stop-color="#2f3fa8"/></linearGradient>` +
    `<linearGradient id="gtx-s" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#fff" stop-opacity=".35"/><stop offset=".5" stop-color="#fff" stop-opacity="0"/></linearGradient></defs>` +
    `<path d="M50 2C14 2 2 14 2 50s12 48 48 48 48-12 48-48S86 2 50 2Z" fill="url(#gtx-g)"/>` +
    `<path d="M50 2C14 2 2 14 2 50s12 48 48 48 48-12 48-48S86 2 50 2Z" fill="url(#gtx-s)"/>` +
    `<text x="50" y="74" text-anchor="middle" font-family="'Times New Roman',Georgia,serif" font-style="italic" font-size="72" fill="#eef2ff">∂</text>`;
  return svg;
}
