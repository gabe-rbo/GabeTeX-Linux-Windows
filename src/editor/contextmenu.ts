// Editor context menu (port of LaTeXTextView.menu(for:) in TextViewSync.swift plus the standard text menu):
// "Show in PDF" (reports the clicked position) on top, then Go to Definition for `\ref`-family keys and include
// arguments, Toggle Comment, and Cut / Copy / Paste / Select All. Loaded with the main bundle (a few hundred bytes);
// the menu widget itself is the shared glass popup menu.
import { EditorSelection } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { popupMenu, type MenuItem } from "../ui/overlay";
import { t } from "../i18n";
import { includeTargetAt, labelKeyAt, type ClickHandlers } from "./mouse";

export interface ContextMenuHost extends ClickHandlers {
  toggleComment(view: EditorView): void;
}

const mod = /Mac|iPhone|iPad/.test(navigator.platform || "") ? "⌘" : "Ctrl+";

/** Position under the pointer, only when it is over a character (not the empty area of the editor). */
function posAt(view: EditorView, x: number, y: number): number | null {
  const pos = view.posAtCoords({ x, y }, false);
  return pos ?? null;
}

export function editorContextMenu(host: () => ContextMenuHost) {
  return EditorView.domEventHandlers({
    contextmenu(event, view) {
      if (event.ctrlKey && event.button !== 2) return false;
      event.preventDefault();
      const pos = posAt(view, event.clientX, event.clientY);
      const sel = view.state.selection.main;
      // Like NSTextView: a right click outside the selection moves the caret there first.
      let at = pos ?? sel.head;
      if (pos !== null && (sel.empty || pos < sel.from || pos > sel.to)) {
        view.dispatch({ selection: EditorSelection.cursor(pos), userEvent: "select.pointer" });
        at = pos;
      }
      const h = host();
      const readOnly = view.state.readOnly;
      const hasSelection = !view.state.selection.main.empty;
      const include = includeTargetAt(view.state, at);
      const label = include ? null : labelKeyAt(view.state, at);
      const items: MenuItem[] = [
        { title: t("Show in PDF"), shortcut: `${mod}⇧J`.replace("Ctrl+⇧", "Ctrl+Shift+"), action: () => h.forwardSync(at) },
        { kind: "separator" },
      ];
      if (include || label) {
        items.push(
          { title: t("Go to Definition"), action: () => (include ? h.openInclude(include) : h.jumpToLabel(label!)) },
          { kind: "separator" },
        );
      }
      items.push(
        { title: t("Toggle Comment"), shortcut: `${mod}/`, disabled: readOnly, action: () => { h.toggleComment(view); view.focus(); } },
        { kind: "separator" },
        { title: t("Cut"), shortcut: `${mod}X`, disabled: readOnly || !hasSelection, action: () => clip(view, "cut") },
        { title: t("Copy"), shortcut: `${mod}C`, disabled: !hasSelection, action: () => clip(view, "copy") },
        { title: t("Paste"), shortcut: `${mod}V`, disabled: readOnly, action: () => void paste(view) },
        { kind: "separator" },
        {
          title: t("Select All"), shortcut: `${mod}A`,
          action: () => { view.dispatch({ selection: EditorSelection.single(0, view.state.doc.length) }); view.focus(); },
        },
      );
      popupMenu(items, { x: event.clientX, y: event.clientY });
      return true;
    },
  });
}

function clip(view: EditorView, kind: "cut" | "copy"): void {
  view.focus();
  // execCommand goes through CodeMirror's copy/cut handlers (multiple ranges, line endings).
  if (!document.execCommand(kind)) {
    const text = view.state.sliceDoc(view.state.selection.main.from, view.state.selection.main.to);
    void navigator.clipboard?.writeText(text).catch(() => undefined);
    if (kind === "cut") view.dispatch(view.state.replaceSelection(""), { userEvent: "delete.cut" });
  }
}

async function paste(view: EditorView): Promise<void> {
  view.focus();
  try {
    const text = await navigator.clipboard.readText();
    if (!text) return;
    view.dispatch(view.state.replaceSelection(text.replace(/\r\n?/g, "\n")), { userEvent: "input.paste", scrollIntoView: true });
  } catch {
    document.execCommand("paste");
  }
}
