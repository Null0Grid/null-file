// White line icons (stroke only). Color and the cyan glow come from CSS, so
// they light up on hover and selection like the rest of Null OS.

const P = {
  dir: '<path d="M3 7.5Q3 6 4.5 6H9.5L11.5 8H19.5Q21 8 21 9.5V17.5Q21 19 19.5 19H4.5Q3 19 3 17.5Z"/><path d="M3 10.5H21"/>',
  dirOpen: '<path d="M3 7.5Q3 6 4.5 6H9.5L11.5 8H18.5Q20 8 20 9.5V10.5"/><path d="M3 18.5L5.6 11.5Q6 10.5 7 10.5H21.2Q22.2 10.5 21.8 11.5L19.5 18Q19.2 19 18.2 19H4Q3 19 3 18Z"/>',
  file: '<path d="M6 3H14L19 8V20Q19 21 18 21H6Q5 21 5 20V4Q5 3 6 3Z"/><path d="M14 3V8H19"/>',
  text: '<path d="M6 3H14L19 8V20Q19 21 18 21H6Q5 21 5 20V4Q5 3 6 3Z"/><path d="M14 3V8H19"/><path d="M8.5 12.5H15.5M8.5 15.5H15.5M8.5 18H12.5"/>',
  code: '<path d="M6 3H14L19 8V20Q19 21 18 21H6Q5 21 5 20V4Q5 3 6 3Z"/><path d="M14 3V8H19"/><path d="M10 12.5L8 15L10 17.5M14 12.5L16 15L14 17.5"/>',
  img: '<rect x="3.5" y="4.5" width="17" height="15" rx="1.5"/><circle cx="9" cy="9.5" r="1.6"/><path d="M4 17.5L9.5 12.5L13 15.5L15.5 13L20 17"/>',
  video: '<rect x="3" y="5.5" width="13" height="13" rx="1.5"/><path d="M16 10.5L21 7.5V16.5L16 13.5"/>',
  audio: '<path d="M9 17.5V5.5L19 3.5V15.5"/><circle cx="6.8" cy="17.5" r="2.3"/><circle cx="16.8" cy="15.5" r="2.3"/>',
  doc: '<path d="M6 3H14L19 8V20Q19 21 18 21H6Q5 21 5 20V4Q5 3 6 3Z"/><path d="M14 3V8H19"/><rect x="8" y="11.5" width="8" height="6" rx=".6"/><path d="M8 14.5H16"/>',
  archive: '<rect x="4" y="3.5" width="16" height="17" rx="1.5"/><path d="M12 3.5V6M12 7.5V9M12 10.5V12"/><rect x="10.3" y="12.5" width="3.4" height="3.4" rx=".5"/>',
  exec: '<rect x="3" y="4.5" width="18" height="15" rx="1.5"/><path d="M7 10L10 12.5L7 15M12 15.5H17"/>',
  link: '<path d="M10 14L14 10"/><path d="M8.5 11.5L6.5 13.5Q4.5 15.5 6.5 17.5Q8.5 19.5 10.5 17.5L12.5 15.5"/><path d="M11.5 8.5L13.5 6.5Q15.5 4.5 17.5 6.5Q19.5 8.5 17.5 10.5L15.5 12.5"/>',
  home: '<path d="M4 11L12 4L20 11"/><path d="M6 9.5V20H18V9.5"/><path d="M10 20V14.5H14V20"/>',
  desktop: '<rect x="3" y="4" width="18" height="12.5" rx="1.5"/><path d="M9 20H15M12 16.5V20"/>',
  docs: '<path d="M6 3H14L19 8V20Q19 21 18 21H6Q5 21 5 20V4Q5 3 6 3Z"/><path d="M14 3V8H19"/><path d="M8.5 12.5H15.5M8.5 15.5H15.5"/>',
  down: '<path d="M12 4V15M7.5 10.5L12 15L16.5 10.5"/><path d="M4.5 19.5H19.5"/>',
  root: '<rect x="4" y="4" width="16" height="6.5" rx="1.2"/><rect x="4" y="13.5" width="16" height="6.5" rx="1.2"/><path d="M7.5 7.25H8.5M7.5 16.75H8.5"/>',
  drive: '<rect x="3.5" y="8" width="17" height="8.5" rx="1.5"/><path d="M7 12.25H7.5M16.5 12.25H17"/><path d="M6.5 8L8.5 4.5H15.5L17.5 8"/>',
  net: '<rect x="8" y="3" width="8" height="6" rx="1"/><rect x="3" y="15" width="7" height="6" rx="1"/><rect x="14" y="15" width="7" height="6" rx="1"/><path d="M12 9V12M6.5 15V12H17.5V15"/>',
  trash: '<path d="M4.5 7H19.5"/><path d="M9.5 7V4.5H14.5V7"/><path d="M6.5 7L7.5 20H16.5L17.5 7"/><path d="M10 10.5V16.5M14 10.5V16.5"/>',
  pin: '<path d="M9 3.5H15L14 9.5L17.5 12.5V14H6.5V12.5L10 9.5Z"/><path d="M12 14V20.5"/>',
  graph: '<circle cx="6" cy="12" r="2.2"/><circle cx="17" cy="6" r="2.2"/><circle cx="17" cy="18" r="2.2"/><path d="M8 11L15 7M8 13L15 17"/>',
};

export const ICONS = P;

export function icon(name, cls = "ic") {
  return `<svg class="${cls}" viewBox="0 0 24 24" aria-hidden="true">${P[name] || P.file}</svg>`;
}

/** Icon for a listing entry. */
export function iconFor(e) {
  if (e.dir) return icon("dir");
  return icon(P[e.kind] ? e.kind : "file");
}
