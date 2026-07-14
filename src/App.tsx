import { FormEvent, KeyboardEvent, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type MediaRecord = {
  id: number; title: string; sourceKind: "remote_url" | "local_file";
  remoteUrl: string | null; storageFilename: string | null; originalFilename: string | null;
  externalUrl: string | null; fileHash: string | null;
  mediaType: string; notes: string; folderNames: string[]; tagNames: string[];
  folderIds: number[]; aliasNames: string[];
  hostedUrl: string | null;
  createdAt: string; useCount: number; lastUsedAt: string | null; hasManualPreview: boolean;
};
type FolderRecord = { id: number; name: string; parentId: number | null; path: string; smart?: boolean };
type ImportPreview = { kind: string; itemCount: number; conflictCount: number; localFileCount: number; remoteUrlCount: number; conflicts: string[]; aliasMatchCount: number; aliasUnmatchedCount: number; aliasUnmatched: string[] };
type PreviewData = { bytes: number[]; mimeType: string };
type EditState = { item: MediaRecord; title: string; notes: string; externalUrl: string; tags: string; aliases: string; folderIds: number[] };
type FolderDeleteState = { folder: FolderRecord; childFolderCount: number; mediaCount: number };
type ContextMenuState = { item: MediaRecord; x: number; y: number };
type FolderContextMenuState = { folder: FolderRecord; x: number; y: number };
type KeybindSettingsView = { showFocusKeybind: string | null; toggleVisibilityKeybind: string | null; showFocusActive: boolean; toggleVisibilityActive: boolean; warnings: string[] };
type ExportSettingsView = { directory: string | null; folderName: string | null; exists: boolean };
type ImportDestination = "current" | "root" | "existing" | "create";
type DeletedMediaSnapshot = { record: MediaRecord; thumbnailFilename: string | null };
type UndoAction =
  | { kind: "remove"; snapshot: DeletedMediaSnapshot }
  | { kind: "removeMany"; snapshots: DeletedMediaSnapshot[] }
  | { kind: "folders"; states: { mediaId: number; folderIds: number[] }[] };

type ImportMode = "url" | "file";
type SortMode = "recent" | "frequent" | "az" | "za";
type ExportScope = "all" | "current";

const emptyForm = { title: "", url: "", mediaType: "gif", notes: "", tags: "", aliases: "", folderIds: [] as number[] };
const UNCATEGORIZED_FOLDER: FolderRecord = { id: -1, name: "Uncategorized", parentId: null, path: "Uncategorized", smart: true };

export function App() {
  const [items, setItems] = useState<MediaRecord[]>([]);
  const [previews, setPreviews] = useState<Record<number, string>>({});
  const [availableLocalIds, setAvailableLocalIds] = useState<Set<number>>(new Set());
  const [brokenPreviews, setBrokenPreviews] = useState<Set<number>>(new Set());
  const [folders, setFolders] = useState<FolderRecord[]>([]);
  const [newFolder, setNewFolder] = useState("");
  const [parentFolder, setParentFolder] = useState("");
  const [importPayload, setImportPayload] = useState("");
  const [importPreview, setImportPreview] = useState<ImportPreview | null>(null);
  const [conflictStrategy, setConflictStrategy] = useState("skip");
  const [globalQuery, setGlobalQuery] = useState("");
  const [viewQuery, setViewQuery] = useState("");
  const [currentFolderId, setCurrentFolderId] = useState<number | null>(null);
  const [sortMode, setSortMode] = useState<SortMode>("recent");
  const [exportScope, setExportScope] = useState<ExportScope>("all");
  const [mode, setMode] = useState<ImportMode>("url");
  const [addDetailsOpen, setAddDetailsOpen] = useState(true);
  const [form, setForm] = useState(emptyForm);
  const [file, setFile] = useState<File | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [editState, setEditState] = useState<EditState | null>(null);
  const [deleteItem, setDeleteItem] = useState<MediaRecord | null>(null);
  const [folderDelete, setFolderDelete] = useState<FolderDeleteState | null>(null);
  const [contextMenu, setContextMenu] = useState<ContextMenuState | null>(null);
  const [folderContextMenu, setFolderContextMenu] = useState<FolderContextMenuState | null>(null);
  const [keybinds, setKeybinds] = useState<KeybindSettingsView | null>(null);
  const [showFocusDraft, setShowFocusDraft] = useState("");
  const [toggleDraft, setToggleDraft] = useState("");
  const [keybindBusy, setKeybindBusy] = useState<"show" | "toggle" | null>(null);
  const [exportSettings, setExportSettings] = useState<ExportSettingsView | null>(null);
  const [importDestination, setImportDestination] = useState<ImportDestination>("current");
  const [importFolderId, setImportFolderId] = useState("");
  const [importNewFolder, setImportNewFolder] = useState("");
  const [selectedMediaIds, setSelectedMediaIds] = useState<Set<number>>(new Set());
  const [bulkFolderChanges, setBulkFolderChanges] = useState<Record<number, boolean>>({});
  const [undoStack, setUndoStack] = useState<UndoAction[]>([]);
  const [exportOpen, setExportOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [wipeOpen, setWipeOpen] = useState(false);
  const [wipeConfirmation, setWipeConfirmation] = useState("");
  const [duplicatePreview, setDuplicatePreview] = useState<{ duplicateGroups: number; membershipRemovals: number; uncategorizedRemovals: number; normalFolderRemovals: number; scopesScanned: number; titleOnlyGroupsSkipped: number; groupReasons: string[] } | null>(null);
  const [organizeOpen, setOrganizeOpen] = useState(false);
  const [showReturnTop, setShowReturnTop] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null);

  async function refresh() {
    const [records, folderRecords, availableIds] = await Promise.all([
      invoke<MediaRecord[]>("list_media"),
      invoke<FolderRecord[]>("list_folders"),
      invoke<number[]>("list_available_local_media_ids"),
    ]);
    setItems(records);
    setFolders(folderRecords);
    setCurrentFolderId((current) => current !== null && current !== UNCATEGORIZED_FOLDER.id && !folderRecords.some((folder) => folder.id === current) ? null : current);
    setAvailableLocalIds(new Set(availableIds));
    const previewBacked = records.filter((item) => item.sourceKind === "local_file" || item.hasManualPreview);
    const next: Record<number, string> = {};
    let failedPreviews = 0;
    await Promise.all(previewBacked.map(async (item) => {
      try {
        const preview = await invoke<PreviewData>("read_media_preview", { id: item.id });
        next[item.id] = URL.createObjectURL(new Blob([new Uint8Array(preview.bytes)], { type: preview.mimeType }));
      } catch (reason) {
        failedPreviews += 1;
        console.warn(`Preview unavailable for media ${item.id}`, reason);
      }
    }));
    setPreviews((previous) => {
      Object.values(previous).forEach(URL.revokeObjectURL);
      return next;
    });
    setBrokenPreviews(new Set(previewBacked.filter((item) => !next[item.id]).map((item) => item.id)));
    if (failedPreviews > 0) setNotice(`${failedPreviews} local preview${failedPreviews === 1 ? " is" : "s are"} unavailable. The media records are still usable.`);
  }

  async function loadKeybinds() {
    const view = await invoke<KeybindSettingsView>("get_keybind_settings");
    setKeybinds(view);
    setShowFocusDraft(view.showFocusKeybind ?? "");
    setToggleDraft(view.toggleVisibilityKeybind ?? "");
  }

  async function loadExportSettings() {
    setExportSettings(await invoke<ExportSettingsView>("get_export_settings"));
  }

  useEffect(() => { Promise.all([refresh(), loadKeybinds(), loadExportSettings()]).catch((reason) => setError(String(reason))); }, []);
  useEffect(() => () => Object.values(previews).forEach(URL.revokeObjectURL), [previews]);
  useEffect(() => {
    if (!notice && !error) return;
    const timeout = window.setTimeout(() => { setNotice(""); setError(""); }, 5_000);
    return () => window.clearTimeout(timeout);
  }, [notice, error]);
  useEffect(() => { setSelectedMediaIds(new Set()); setBulkFolderChanges({}); }, [currentFolderId]);
  useEffect(() => { setBulkFolderChanges({}); }, [selectedMediaIds]);
  useEffect(() => {
    const update = () => setShowReturnTop(window.scrollY > 300);
    window.addEventListener("scroll", update, { passive: true });
    return () => window.removeEventListener("scroll", update);
  }, []);

  const currentFolder = currentFolderId === UNCATEGORIZED_FOLDER.id ? UNCATEGORIZED_FOLDER : folders.find((folder) => folder.id === currentFolderId) ?? null;
  const visibleFolders = useMemo(() => {
    const children = currentFolderId === null
      ? [UNCATEGORIZED_FOLDER, ...folders.filter((folder) => folder.parentId === null)]
      : currentFolderId === UNCATEGORIZED_FOLDER.id
        ? []
        : folders.filter((folder) => folder.parentId === currentFolderId);
    return [...children].sort((a, b) => {
      if (a.smart !== b.smart) return a.smart ? -1 : 1;
      return sortMode === "za" ? b.name.localeCompare(a.name) : a.name.localeCompare(b.name);
    });
  }, [folders, currentFolderId, sortMode]);
  const activeGlobalSearch = globalQuery.trim().length > 0;
  const activeNeedle = (activeGlobalSearch ? globalQuery : viewQuery).trim().toLocaleLowerCase();
  const displayedFolders = activeGlobalSearch ? [] : visibleFolders;
  const visibleFolderIds = useMemo(() => new Set(visibleFolders.map((folder) => folder.id)), [visibleFolders]);
  const matchingFolderResults = useMemo(() => {
    if (!activeNeedle) return [];
    const scopedFolders = activeGlobalSearch
      ? folders
      : currentFolderId === null
        ? folders
        : currentFolderId === UNCATEGORIZED_FOLDER.id
          ? []
          : folders.filter((folder) => folder.id !== currentFolderId && isFolderDescendantOf(folder, currentFolderId, folders));
    return scopedFolders
      .filter((folder) => activeGlobalSearch || !visibleFolderIds.has(folder.id))
      .filter((folder) => [folder.name, folder.path].some((value) => value.toLocaleLowerCase().includes(activeNeedle)))
      .sort((a, b) => a.path.localeCompare(b.path));
  }, [activeGlobalSearch, activeNeedle, currentFolderId, folders, visibleFolderIds]);
  const filtered = useMemo(() => {
    const inView = activeGlobalSearch ? items : currentFolderId === null
      ? items
      : currentFolderId === UNCATEGORIZED_FOLDER.id
        ? items.filter((item) => item.folderIds.length === 0)
        : items.filter((item) => item.folderIds.includes(currentFolderId));
    const matches = activeNeedle ? inView.filter((item) => [item.title, item.notes, item.remoteUrl ?? "", item.externalUrl ?? "", item.hostedUrl ?? "", item.originalFilename ?? "", ...item.folderNames, ...item.tagNames, ...item.aliasNames]
      .some((value) => value.toLocaleLowerCase().includes(activeNeedle))) : inView;
    return [...matches].sort((a, b) => {
      if (sortMode === "az") return a.title.localeCompare(b.title);
      if (sortMode === "za") return b.title.localeCompare(a.title);
      if (sortMode === "frequent") return b.useCount - a.useCount || a.title.localeCompare(b.title);
      const aDate = a.lastUsedAt ?? a.createdAt;
      const bDate = b.lastUsedAt ?? b.createdAt;
      return bDate.localeCompare(aDate) || a.title.localeCompare(b.title);
    });
  }, [items, activeGlobalSearch, activeNeedle, currentFolderId, sortMode]);

  const breadcrumbs = useMemo(() => {
    const path: FolderRecord[] = [];
    let cursor = currentFolder;
    while (cursor) {
      path.unshift(cursor);
      cursor = cursor.smart ? null : folders.find((folder) => folder.id === cursor?.parentId) ?? null;
    }
    return path;
  }, [currentFolder, folders]);

  function folderMediaCount(folder: FolderRecord) {
    return folder.smart
      ? items.filter((item) => item.folderIds.length === 0).length
      : items.filter((item) => item.folderIds.includes(folder.id)).length;
  }

  async function submit(event: FormEvent) {
    event.preventDefault(); setBusy(true); setError(""); setNotice("");
    let thumbnailWarning = false;
    const tagNames = form.tags.split(",").map((tag) => tag.trim()).filter(Boolean);
    const aliasNames = form.aliases.split(",").map((alias) => alias.trim()).filter(Boolean);
    try {
      const rawUrl = form.url.trim();
      if (!rawUrl && !file) throw new Error("Add a URL, choose a local image file, or provide both.");
      let link: string | null = null;
      if (rawUrl) {
        const url = new URL(rawUrl);
        if (!/^https?:$/.test(url.protocol)) throw new Error("The URL must use http:// or https://.");
        link = url.toString();
      }
      if (!file) {
        await invoke("create_media", { input: {
          title: form.title, sourceKind: "remote_url", remoteUrl: link, externalUrl: link, storageFilename: null,
          originalFilename: null, mediaType: form.mediaType, fileHash: null, notes: form.notes,
          folderNames: [], folderIds: form.folderIds, tagNames, aliasNames,
        }});
      } else {
        const mediaType = fileType(file);
        const bytes = await file.arrayBuffer();
        let thumbnailBytes = new Uint8Array();
        const needsThumbnail = mediaType === "gif" || mediaType === "webp" || (mediaType === "png" && isAnimatedPng(new Uint8Array(bytes)));
        if (needsThumbnail) {
          try { thumbnailBytes = await makeThumbnail(file); }
          catch (reason) { thumbnailWarning = true; console.warn("Thumbnail generation failed; importing without one", reason); }
        }
        const input = {
          title: form.title, originalFilename: file.name, mediaType, externalUrl: link,
          bytes: Array.from(new Uint8Array(bytes)), thumbnailBytes: Array.from(thumbnailBytes),
          notes: form.notes, folderNames: [], folderIds: form.folderIds, tagNames, aliasNames, importAnyway: false,
        };
        try { await invoke("import_local_media", { input }); }
        catch (reason) {
          const message = String(reason);
          if (!message.startsWith("DUPLICATE_FILE:")) throw reason;
          const existing = message.slice("DUPLICATE_FILE:".length);
          if (!window.confirm(`This file matches "${existing}". Import another copy anyway?`)) { setNotice("Duplicate import skipped."); return; }
          await invoke("import_local_media", { input: { ...input, importAnyway: true } });
        }
      }
      setForm(emptyForm); setFile(null); if (fileInput.current) fileInput.current.value = "";
      await refresh(); setNotice(thumbnailWarning ? "Added to your library, but a preview could not be generated." : "Added to your library.");
    } catch (reason) { setAddDetailsOpen(true); setError(reason instanceof Error ? reason.message : String(reason)); }
    finally { setBusy(false); }
  }

  async function addFolder() {
    setError("");
    try {
      const created = await invoke<FolderRecord>("create_folder", { name: newFolder, parentId: parentFolder ? Number(parentFolder) : null });
      setNewFolder(""); setNotice(`Created folder "${created.path}".`); await refresh();
    } catch (reason) { setError(String(reason)); }
  }

  function toggleFolder(id: number) {
    setForm((current) => ({ ...current, folderIds: current.folderIds.includes(id) ? current.folderIds.filter((value) => value !== id) : [...current.folderIds, id] }));
  }

  async function exportFile(kind: "library" | "aliases") {
    setError("");
    try {
      const folderId = exportScope === "current" ? currentFolderId : null;
      const date = new Date().toISOString().slice(0, 10);
      const scopedName = folderId ? currentFolder?.name ?? null : null;
      let destination = "Downloads";
      if (exportSettings?.directory) {
        if (!exportSettings.exists) throw new Error("The configured export directory no longer exists. Choose a new export location in Settings.");
        const result = await invoke<{ folderName: string; filename: string }>("export_to_configured_directory", { kind, folderId, folderName: scopedName, date });
        destination = result.folderName;
      } else {
        const payload = await invoke<string>(kind === "library" ? "export_library" : "export_aliases", { folderId });
        downloadJson(payload, exportFilename(kind, scopedName, date));
      }
      const exported = kind === "aliases"
        ? scopedName ? `aliases for '${scopedName}'` : "all aliases"
        : scopedName ? `'${scopedName}'` : "full library";
      setNotice(`Exported ${exported} to ${destination}.`);
      setExportOpen(false);
    } catch (reason) { setError(`Export failed: ${reason instanceof Error ? reason.message : String(reason)}`); }
  }

  async function chooseExportDirectory() {
    try { setExportSettings(await invoke<ExportSettingsView>("choose_export_directory")); setError(""); }
    catch (reason) { setError(`Could not choose export location: ${String(reason)}`); }
  }

  async function clearExportDirectory() {
    try { setExportSettings(await invoke<ExportSettingsView>("clear_export_directory")); setNotice("Export location reset to default behavior."); }
    catch (reason) { setError(`Could not clear export location: ${String(reason)}`); }
  }

  async function selectImport(file: File | null) {
    setImportPreview(null); setImportPayload(""); setError("");
    if (!file) return;
    setImportDestination("current"); setImportFolderId(""); setImportNewFolder("");
    try {
      const payload = await file.text();
      const preview = await invoke<ImportPreview>("preview_import", { payload });
      setImportPayload(payload); setImportPreview(preview);
    } catch (reason) { setError(String(reason)); }
  }

  function closeImport() {
    if (busy) return;
    setImportPayload("");
    setImportPreview(null);
  }

  async function applyTransfer() {
    if (!importPreview) return;
    setBusy(true); setError("");
    try {
      let destinationFolderId: number | null = null;
      let destinationLabel = "current library";
      if (importDestination === "current" && currentFolder) {
        destinationFolderId = currentFolder.smart && importPreview.kind === "aliases" ? currentFolder.id : currentFolder.smart ? null : currentFolder.id;
        destinationLabel = currentFolder.smart ? "Uncategorized" : `'${currentFolder.name}'`;
      }
      else if (importDestination === "existing") {
        const folder = folders.find((value) => value.id === Number(importFolderId));
        if (!folder) throw new Error("Choose an existing destination folder.");
        destinationFolderId = folder.id; destinationLabel = `'${folder.name}'`;
      } else if (importDestination === "create" && importPreview.kind !== "aliases") {
        if (!importNewFolder.trim()) throw new Error("Enter a name for the new destination folder.");
        const folder = await invoke<FolderRecord>("create_folder", { name: importNewFolder, parentId: currentFolder?.smart ? null : currentFolderId });
        destinationFolderId = folder.id; destinationLabel = `'${folder.name}'`;
      }
      const result = await invoke<{ imported: number; skipped: number; importedFiles: number; importedLinks: number; skippedDuplicates: number; skippedUnsupported: number; unmatchedAliases: string[]; aliasMatchedRecords: number; aliasesAdded: number; aliasesSkipped: number }>("apply_import", { payload: importPayload, conflictStrategy, destinationFolderId });
      if (importPreview.kind === "aliases") {
        const scope = destinationFolderId === null ? "the full library" : destinationLabel;
        const unmatched = result.unmatchedAliases.length;
        const message = result.aliasesAdded === 0
          ? `No matching media with new aliases found in ${scope}. No aliases were imported. ${result.aliasMatchedRecords} matched record${result.aliasMatchedRecords === 1 ? "" : "s"}; ${result.aliasesSkipped} aliases skipped; ${unmatched} unmatched item${unmatched === 1 ? "" : "s"}.`
          : `Imported ${result.aliasesAdded} alias${result.aliasesAdded === 1 ? "" : "es"} into ${scope}. ${result.aliasMatchedRecords} matched record${result.aliasMatchedRecords === 1 ? "" : "s"}; ${result.aliasesSkipped} aliases skipped; ${unmatched} unmatched item${unmatched === 1 ? "" : "s"}.`;
        setNotice(message); setImportPayload(""); setImportPreview(null); await refresh();
        return;
      }
      const parts = [];
      if (result.importedFiles) parts.push(`${result.importedFiles} file${result.importedFiles === 1 ? "" : "s"}`);
      if (result.importedLinks) parts.push(`${result.importedLinks} link${result.importedLinks === 1 ? "" : "s"}`);
      if (!parts.length) parts.push(`${result.imported} alias record${result.imported === 1 ? "" : "s"}`);
      let message = `Imported ${parts.join(" and ")} to ${destinationLabel}.`;
      const skippedParts = [];
      if (result.skippedDuplicates) skippedParts.push(`${result.skippedDuplicates} duplicate${result.skippedDuplicates === 1 ? "" : "s"}`);
      if (result.skippedUnsupported) skippedParts.push(`${result.skippedUnsupported} unsupported item${result.skippedUnsupported === 1 ? "" : "s"}`);
      if (result.unmatchedAliases.length) skippedParts.push(`${result.unmatchedAliases.length} unmatched alias record${result.unmatchedAliases.length === 1 ? "" : "s"}`);
      if (skippedParts.length) message += ` Skipped ${skippedParts.join(" and ")}.`;
      setNotice(message); setImportPayload(""); setImportPreview(null); await refresh();
    } catch (reason) { setError(`Import failed: ${reason instanceof Error ? reason.message : String(reason)}`); } finally { setBusy(false); }
  }

  function beginEdit(item: MediaRecord) {
    setEditState({ item, title: item.title, notes: item.notes, externalUrl: item.externalUrl ?? item.remoteUrl ?? "", tags: item.tagNames.join(", "), aliases: item.aliasNames.join(", "), folderIds: item.folderIds });
  }

  async function saveEdit() {
    if (!editState) return;
    const { item } = editState;
    setBusy(true); setError("");
    try {
      const url = editState.externalUrl.trim() || null;
      await invoke("update_media", { id: item.id, input: {
        title: editState.title, sourceKind: item.sourceKind,
        remoteUrl: item.sourceKind === "remote_url" ? item.remoteUrl ?? url : null,
        externalUrl: url,
        storageFilename: item.storageFilename, originalFilename: item.originalFilename,
        mediaType: item.mediaType, fileHash: item.fileHash, notes: editState.notes,
        folderNames: [], folderIds: editState.folderIds,
        tagNames: editState.tags.split(",").map((value) => value.trim()).filter(Boolean), aliasNames: editState.aliases.split(",").map((value) => value.trim()).filter(Boolean),
      }});
      setEditState(null); await refresh(); setNotice(`Updated "${editState.title}".`);
    } catch (reason) { setError(String(reason)); } finally { setBusy(false); }
  }

  async function confirmDeleteMedia() {
    if (!deleteItem) return;
    try {
      const snapshot = await invoke<DeletedMediaSnapshot>("delete_media_for_undo", { id: deleteItem.id });
      setUndoStack((current) => [...current, { kind: "remove", snapshot } as UndoAction].slice(-3));
      setDeleteItem(null); await refresh(); setNotice("Removed from the library. Use Undo to restore it; managed files were left untouched.");
    } catch (reason) { setError(String(reason)); }
  }

  async function renameExistingFolder(folder: FolderRecord) {
    const name = window.prompt("Rename folder", folder.name);
    if (name === null || !name.trim()) return;
    try { await invoke("rename_folder", { id: folder.id, name }); await refresh(); setNotice("Folder renamed."); }
    catch (reason) { setError(String(reason)); }
  }

  async function inspectFolderDelete(folder: FolderRecord) {
    try {
      const impact = await invoke<{ childFolderCount: number; mediaCount: number }>("folder_delete_impact", { id: folder.id });
      setFolderDelete({ folder, ...impact });
    } catch (reason) { setError(String(reason)); }
  }

  async function confirmFolderDelete(mode: "delete_subtree" | "reparent") {
    if (!folderDelete) return;
    try { await invoke("delete_folder", { id: folderDelete.folder.id, mode }); setFolderDelete(null); await refresh(); setNotice("Folder removed. Media records were not deleted."); }
    catch (reason) { setError(String(reason)); }
  }

  function itemLink(item: MediaRecord) {
    return item.hostedUrl ?? item.externalUrl ?? item.remoteUrl;
  }

  function hasLocalFile(item: MediaRecord) {
    return item.sourceKind === "local_file" && !!item.storageFilename && availableLocalIds.has(item.id);
  }

  function itemPreview(item: MediaRecord) {
    return previews[item.id] ?? item.externalUrl ?? item.remoteUrl ?? undefined;
  }

  async function uploadPreview(item: MediaRecord) {
    setContextMenu(null); setError(""); setNotice("");
    if ((item.hasManualPreview || (item.sourceKind === "local_file" && !!previews[item.id])) && !window.confirm("Replace the existing preview for this media item?")) return;
    const input = document.createElement("input");
    input.type = "file";
    input.accept = ".gif,.png,.jpg,.jpeg,.webp,image/gif,image/png,image/jpeg,image/webp";
    input.onchange = async () => {
      const previewFile = input.files?.[0];
      if (!previewFile) return;
      try {
        const mediaType = fileType(previewFile);
        const bytes = await previewFile.arrayBuffer();
        await invoke("upload_manual_preview", { id: item.id, input: { mediaType, bytes: Array.from(new Uint8Array(bytes)) } });
        await refresh();
        setNotice(`Uploaded preview for "${item.title}".`);
      } catch (reason) {
        setError(`Could not upload preview: ${reason instanceof Error ? reason.message : String(reason)}`);
      }
    };
    input.click();
  }

  async function clearManualPreview(item: MediaRecord) {
    setError(""); setNotice("");
    if (!item.hasManualPreview) return;
    try {
      await invoke("clear_manual_preview", { id: item.id });
      await refresh();
      setNotice(`Cleared uploaded preview for "${item.title}".`);
    } catch (reason) { setError(`Could not clear uploaded preview: ${String(reason)}`); }
  }

  async function copyLink(item: MediaRecord) {
    setError(""); setNotice("");
    const link = itemLink(item);
    if (!link) return;
    try { await navigator.clipboard.writeText(link); await recordUsed(item); setNotice(`Copied "${item.title}" link.`); }
    catch (reason) { setError(`Could not copy the link: ${reason instanceof Error ? reason.message : String(reason)}`); }
  }

  async function copyFile(item: MediaRecord) {
    setError(""); setNotice("");
    if (!hasLocalFile(item)) return;
    try { await invoke("copy_local_media_file", { id: item.id }); await recordUsed(item); setNotice(`Copied "${item.title}" as a file.`); }
    catch (reason) { setError(`Could not copy the file: ${String(reason)}`); }
  }

  async function activate(item: MediaRecord) {
    if (itemLink(item)) await copyLink(item);
    else if (hasLocalFile(item)) await copyFile(item);
  }

  async function recordUsed(item: MediaRecord) {
    try {
      await invoke("record_media_used", { id: item.id });
      const now = new Date().toISOString();
      setItems((current) => current.map((value) => value.id === item.id ? { ...value, useCount: value.useCount + 1, lastUsedAt: now } : value));
    } catch (reason) {
      console.warn(`Usage update failed for media ${item.id}`, reason);
    }
  }

  async function copyLabels(kind: "tags" | "aliases", values: string[]) {
    try {
      await navigator.clipboard.writeText(values.join(", "));
      setError("");
      setNotice(kind === "aliases" ? "Copied aliases." : "Copied all tags.");
    } catch (reason) {
      setError(`Could not copy ${kind}: ${reason instanceof Error ? reason.message : String(reason)}`);
    }
  }

  function toggleMediaSelection(id: number) {
    setSelectedMediaIds((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id); else next.add(id);
      return next;
    });
  }

  function bulkFolderState(id: number) {
    const selected = items.filter((item) => selectedMediaIds.has(item.id));
    const assigned = selected.filter((item) => item.folderIds.includes(id)).length;
    if (Object.prototype.hasOwnProperty.call(bulkFolderChanges, id)) {
      return { checked: bulkFolderChanges[id], indeterminate: false };
    }
    return {
      checked: selected.length > 0 && assigned === selected.length,
      indeterminate: assigned > 0 && assigned < selected.length,
    };
  }

  function changeBulkFolder(id: number, assigned: boolean) {
    setBulkFolderChanges((current) => ({ ...current, [id]: assigned }));
  }

  async function applyBulkFolders() {
    setBusy(true); setError(""); setNotice("");
    try {
      const changes = Object.entries(bulkFolderChanges).map(([folderId, assigned]) => ({ folderId: Number(folderId), assigned }));
      const movingToUncategorized = items.filter((item) => selectedMediaIds.has(item.id)).filter((item) => {
        const finalFolders = new Set(item.folderIds);
        for (const change of changes) {
          if (change.assigned) finalFolders.add(change.folderId); else finalFolders.delete(change.folderId);
        }
        return item.folderIds.length > 0 && finalFolders.size === 0;
      }).length;
      if (movingToUncategorized > 0 && !window.confirm(`${movingToUncategorized} selected item${movingToUncategorized === 1 ? " will" : "s will"} have no normal folders and move to Uncategorized. Apply these folder changes?`)) return;
      const previousStates = items.filter((item) => selectedMediaIds.has(item.id)).map((item) => ({ mediaId: item.id, folderIds: [...item.folderIds] }));
      await invoke("apply_media_folder_changes", { mediaIds: [...selectedMediaIds], folderChanges: changes });
      setUndoStack((current) => [...current, { kind: "folders", states: previousStates } as UndoAction].slice(-3));
      const count = selectedMediaIds.size;
      const changedFolders = changes.length;
      setSelectedMediaIds(new Set()); setBulkFolderChanges({});
      await refresh();
      setNotice(`Applied ${changedFolders} folder change${changedFolders === 1 ? "" : "s"} to ${count} media item${count === 1 ? "" : "s"}. Use Undo to restore the previous memberships.`);
    } catch (reason) { setError(`Could not organize selected media: ${String(reason)}`); }
    finally { setBusy(false); }
  }

  async function undoLastAction() {
    const undoAction = undoStack.at(-1);
    if (!undoAction) return;
    setBusy(true); setError(""); setNotice("");
    try {
      if (undoAction.kind === "remove") {
        await invoke("restore_deleted_media", { snapshot: undoAction.snapshot });
        setNotice("Restored the removed media item.");
      } else if (undoAction.kind === "removeMany") {
        for (const snapshot of undoAction.snapshots) await invoke("restore_deleted_media", { snapshot });
        setNotice("Restored the removed media items.");
      } else {
        await invoke("restore_media_folder_memberships", { states: undoAction.states });
        setNotice("Restored the previous folder memberships.");
      }
      setUndoStack((current) => current.slice(0, -1));
      await refresh();
    } catch (reason) {
      setError(`Undo failed: ${String(reason)}`);
    } finally {
      setBusy(false);
    }
  }

  useEffect(() => {
    const handleUndo = (event: globalThis.KeyboardEvent) => {
      if (!event.ctrlKey || event.altKey || event.metaKey || event.shiftKey || event.key.toLowerCase() !== "z") return;
      const target = event.target;
      if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement || (target instanceof HTMLElement && target.isContentEditable)) return;
      if (undoStack.length === 0 || busy) return;
      event.preventDefault();
      void undoLastAction();
    };
    window.addEventListener("keydown", handleUndo);
    return () => window.removeEventListener("keydown", handleUndo);
  }, [undoStack, busy]);

  async function wipeAll() {
    if (wipeConfirmation !== "Reset") return;
    setBusy(true); setError(""); setNotice("");
    try {
      const result = await invoke<{ cleanupFailures: string[] }>("wipe_library", { confirmation: wipeConfirmation });
      setWipeOpen(false); setSettingsOpen(false); setWipeConfirmation(""); setUndoStack([]);
      setImportPayload(""); setImportPreview(null); setCurrentFolderId(null);
      await Promise.all([refresh(), loadKeybinds(), loadExportSettings()]);
      if (result.cleanupFailures.length) setError(`Library records were wiped, but cleanup was incomplete: ${result.cleanupFailures.join("; ")}.`);
      else setNotice("Library wiped");
    } catch (reason) { setError(`Library wipe failed: ${String(reason)}`); }
    finally { setBusy(false); }
  }

  async function uninstallApp() {
    try { setNotice(await invoke<string>("open_uninstall")); }
    catch (reason) { setError(String(reason)); }
  }

  async function purgeStaticThumbnails() {
    setBusy(true); setError(""); setNotice("");
    try {
      const result = await invoke<{ purged: number; cleanupFailures: number }>("purge_static_image_thumbnails");
      await refresh();
      if (result.purged === 0) setNotice("No static thumbnails to purge.");
      else if (result.cleanupFailures > 0) setNotice("Purged static image thumbnails. Some files could not be removed.");
      else setNotice("Purged static image thumbnails");
    } catch (reason) { setError(`Could not purge static image thumbnails: ${String(reason)}`); }
    finally { setBusy(false); }
  }

  async function inspectDuplicatePurge() {
    try {
      const preview = await invoke<{ duplicateGroups: number; membershipRemovals: number; uncategorizedRemovals: number; normalFolderRemovals: number; scopesScanned: number; titleOnlyGroupsSkipped: number; groupReasons: string[] }>("preview_duplicate_purge");
      if (preview.membershipRemovals === 0) {
        const skipped = preview.titleOnlyGroupsSkipped ? ` Possible title-only duplicates were not purged automatically: ${preview.titleOnlyGroupsSkipped}.` : "";
        setNotice(`No duplicates found using file hash, URL, local file path, and safe title+metadata matching.${skipped}`);
      }
      else setDuplicatePreview(preview);
    } catch (reason) { setError(`Could not inspect duplicates: ${String(reason)}`); }
  }

  async function purgeDuplicates() {
    setBusy(true); setError(""); setNotice("");
    try {
      const result = await invoke<{ duplicateGroups: number; membershipRemovals: number; uncategorizedRemovals: number; normalFolderRemovals: number; scopesScanned: number; titleOnlyGroupsSkipped: number; groupReasons: string[]; previousStates: { mediaId: number; folderIds: number[] }[]; deletedSnapshots: DeletedMediaSnapshot[] }>("purge_duplicate_folder_items");
      const undoActions: UndoAction[] = [];
      if (result.previousStates.length > 0) undoActions.push({ kind: "folders", states: result.previousStates });
      if (result.deletedSnapshots.length > 0) undoActions.push({ kind: "removeMany", snapshots: result.deletedSnapshots });
      if (undoActions.length > 0) setUndoStack((current) => [...current, ...undoActions].slice(-3));
      const skipped = result.titleOnlyGroupsSkipped ? ` Possible title-only duplicates were not purged automatically: ${result.titleOnlyGroupsSkipped}.` : "";
      setDuplicatePreview(null); await refresh(); setNotice(result.membershipRemovals ? `Purged duplicate folder items.${skipped}` : `No duplicates found using file hash, URL, local file path, and safe title+metadata matching.${skipped}`);
    } catch (reason) { setError(`Could not purge duplicates: ${String(reason)}`); }
    finally { setBusy(false); }
  }

  useEffect(() => {
    const closeTopOverlay = (event: globalThis.KeyboardEvent) => {
      if (event.key !== "Escape") return;
      if (wipeOpen) { setWipeOpen(false); setWipeConfirmation(""); }
      else if (duplicatePreview) setDuplicatePreview(null);
      else if (deleteItem) setDeleteItem(null);
      else if (folderDelete) setFolderDelete(null);
      else if (editState) setEditState(null);
      else if (importPreview) closeImport();
      else if (settingsOpen) setSettingsOpen(false);
      else if (exportOpen) setExportOpen(false);
      else if (contextMenu) setContextMenu(null);
      else if (folderContextMenu) setFolderContextMenu(null);
      else if (organizeOpen) setOrganizeOpen(false);
      else return;
      event.preventDefault();
    };
    window.addEventListener("keydown", closeTopOverlay);
    return () => window.removeEventListener("keydown", closeTopOverlay);
  }, [wipeOpen, duplicatePreview, deleteItem, folderDelete, editState, importPreview, settingsOpen, exportOpen, contextMenu, folderContextMenu, organizeOpen, busy]);

  async function saveKeybind(kind: "show" | "toggle", clear = false) {
    setKeybindBusy(kind); setError(""); setNotice("");
    try {
      const shortcut = clear ? null : (kind === "show" ? showFocusDraft : toggleDraft).trim();
      const view = await invoke<KeybindSettingsView>(kind === "show" ? "set_show_focus_keybind" : "set_toggle_visibility_keybind", { shortcut });
      setKeybinds(view);
      setShowFocusDraft(view.showFocusKeybind ?? "");
      setToggleDraft(view.toggleVisibilityKeybind ?? "");
      setNotice(clear ? "Keybind cleared." : "Keybind registered and saved.");
    } catch (reason) {
      setError(String(reason));
      if (keybinds) {
        setShowFocusDraft(keybinds.showFocusKeybind ?? "");
        setToggleDraft(keybinds.toggleVisibilityKeybind ?? "");
      }
    } finally { setKeybindBusy(null); }
  }

  function recordKeybind(event: KeyboardEvent<HTMLInputElement>, kind: "show" | "toggle") {
    event.preventDefault(); event.stopPropagation();
    const modifiers = [];
    if (event.ctrlKey) modifiers.push("Ctrl");
    if (event.altKey) modifiers.push("Alt");
    if (event.shiftKey) modifiers.push("Shift");
    if (event.metaKey) modifiers.push("Super");
    if (["Control", "Alt", "Shift", "Meta"].includes(event.key)) {
      (kind === "show" ? setShowFocusDraft : setToggleDraft)(modifiers.join("+"));
      return;
    }
    let key = event.code;
    if (key.startsWith("Key")) key = key.slice(3);
    else if (key.startsWith("Digit")) key = key.slice(5);
    else if (key === "Space") key = "Space";
    else if (!key) key = event.key.length === 1 ? event.key.toUpperCase() : event.key;
    (kind === "show" ? setShowFocusDraft : setToggleDraft)([...modifiers, key].join("+"));
  }

  return <main className="app-shell">
    <header className="topbar">
      <div className="topbar-title"><h1 className="eyebrow">Local Media Library</h1><p className="hidden-title-space" aria-hidden="true">Reaction shelf</p></div>
      <div className="topbar-actions"><label className="search"><span>Global search</span><input value={globalQuery} onChange={(e) => setGlobalQuery(e.target.value)} placeholder="Global search..." /></label><label className="search"><span>Current view search</span><input value={viewQuery} onChange={(e) => setViewQuery(e.target.value)} placeholder="Search current view..." disabled={activeGlobalSearch} /></label><button className="settings-button" aria-label="Open settings" title="Settings" onClick={() => setSettingsOpen(true)}>⚙</button></div>
    </header>

    <div className="workspace">
      <aside className="add-panel">
        <section className="add-media-section">
        <h2>Add to Library</h2>
        <div className="segmented" aria-label="Add media source">
          <button type="button" className={mode === "url" ? "active" : ""} onClick={() => setMode("url")}>Remote URL</button>
          <button type="button" className={mode === "file" ? "active" : ""} onClick={() => setMode("file")}>Local file</button>
        </div>
        <details className="add-details" open={addDetailsOpen} onToggle={(event) => setAddDetailsOpen(event.currentTarget.open)}>
          <summary>Add details</summary>
          <form onSubmit={submit}>
          <label>Title<input required value={form.title} onChange={(e) => setForm({...form, title: e.target.value})} placeholder="Confused hamster" /></label>
          {mode === "url" ? <>
            <label>Image URL<input type="url" value={form.url} onChange={(e) => setForm({...form, url: e.target.value})} placeholder="https://.../reaction.gif" /></label>
            <label>Format<select value={form.mediaType} onChange={(e) => setForm({...form, mediaType: e.target.value})}><option>gif</option><option>png</option><option>jpg</option><option>jpeg</option><option>webp</option></select></label>
            <p className="privacy-note">Remote previews contact the image host from this computer.</p>
            {file && <p className="input-status">Local file also selected: {file.name}</p>}
          </> : <><label className="file-picker">Image file<input ref={fileInput} type="file" accept=".gif,.png,.jpg,.jpeg,.webp,image/gif,image/png,image/jpeg,image/webp" onChange={(e) => setFile(e.target.files?.[0] ?? null)} /><span>{file?.name ?? "Choose GIF, PNG, JPG, or WEBP"}</span></label>{form.url.trim() && <p className="input-status">Link also supplied: {form.url.trim()}</p>}</>}
          <fieldset className="folder-picker"><legend>Folders</legend>{folders.length ? folders.map((folder) => <label key={folder.id}><input type="checkbox" checked={form.folderIds.includes(folder.id)} onChange={() => toggleFolder(folder.id)} /><span>{folder.path}</span></label>) : <p>No folders yet.</p>}</fieldset>
          <div className="two-col"><label>Tags<input value={form.tags} onChange={(e) => setForm({...form, tags: e.target.value})} placeholder="mood, reply" /></label><label>Aliases<input value={form.aliases} onChange={(e) => setForm({...form, aliases: e.target.value})} placeholder="confused, huh" /></label></div>
          <label>Notes<textarea value={form.notes} onChange={(e) => setForm({...form, notes: e.target.value})} rows={2} /></label>
          <button className="primary" disabled={busy}>{busy ? "Adding..." : "Add to library"}</button>
          </form>
        </details>
        </section>
        <section className="folder-builder"><h2>Folders</h2><div className="folder-list">{folders.map((folder) => <div key={folder.id}><span>{folder.path}</span><button onClick={() => renameExistingFolder(folder)}>Rename</button><button onClick={() => inspectFolderDelete(folder)}>Delete</button></div>)}</div><label>New folder name<input value={newFolder} onChange={(e) => setNewFolder(e.target.value)} placeholder="Reactions" /></label><label>Inside<select value={parentFolder} onChange={(e) => setParentFolder(e.target.value)}><option value="">Top level</option>{folders.map((folder) => <option key={folder.id} value={folder.id}>{folder.path}</option>)}</select></label><button className="secondary" type="button" disabled={!newFolder.trim()} onClick={addFolder}>Create folder</button></section>
        <section className="transfer-panel"><h2>Backup &amp; share</h2><button className="secondary" onClick={() => setExportOpen(true)}>Export</button><label className="file-picker">Import JSON<input type="file" accept=".json,application/json" onChange={(event) => selectImport(event.target.files?.[0] ?? null)} /><span>Choose export file</span></label></section>
      </aside>

      <section className="library" aria-live="polite">
        <div className="library-heading"><div><h2>{currentFolder?.name ?? "Your media"}</h2><p>{visibleFolders.length} folder{visibleFolders.length === 1 ? "" : "s"} · {filtered.length} item{filtered.length === 1 ? "" : "s"}</p></div><div className="library-tools"><button className="secondary undo-button" disabled={busy || undoStack.length === 0} onClick={undoLastAction}>Undo{undoStack.length ? ` (${undoStack.length})` : ""}</button><label className="sort-control">Sort by<select value={sortMode} onChange={(event) => setSortMode(event.target.value as SortMode)}><option value="recent">Recently used</option><option value="frequent">Frequently used</option><option value="az">A-Z</option><option value="za">Z-A</option></select></label></div></div>
        {currentFolder && <nav className="breadcrumbs" aria-label="Folder navigation">{breadcrumbs.map((folder, index) => <span key={folder.id}>{index > 0 ? "/ " : ""}<button onClick={() => setCurrentFolderId(folder.id)}>{folder.name}</button></span>)}</nav>}
        {currentFolder && <div className="folder-navigation-actions"><button className="back-button" onClick={() => setCurrentFolderId(currentFolder.parentId)}>← Back</button><button className="back-button" onClick={() => setCurrentFolderId(null)}>⌂ Home</button></div>}
        {currentFolderId !== null && <details className="bulk-organizer" open={organizeOpen} onToggle={(event) => setOrganizeOpen(event.currentTarget.open)}><summary>Organize media <span>{selectedMediaIds.size} selected</span></summary><div className="bulk-content"><div><button onClick={() => setSelectedMediaIds(selectedMediaIds.size === filtered.length ? new Set() : new Set(filtered.map((item) => item.id)))}>{selectedMediaIds.size === filtered.length && filtered.length > 0 ? "Clear selection" : "Select all in view"}</button></div><fieldset><legend>Folder memberships for selected items</legend>{folders.map((folder) => { const state = bulkFolderState(folder.id); return <label key={folder.id}><input type="checkbox" checked={state.checked} ref={(node) => { if (node) node.indeterminate = state.indeterminate; }} onChange={(event) => changeBulkFolder(folder.id, event.target.checked)} />{folder.path}</label>; })}</fieldset><button className="primary" disabled={busy || selectedMediaIds.size === 0 || Object.keys(bulkFolderChanges).length === 0} onClick={applyBulkFolders}>Apply folder changes</button></div></details>}
        {displayedFolders.length > 0 && <div className="folder-grid">{displayedFolders.map((folder) => <button className={`folder-card ${folder.smart ? "smart" : ""}`} key={folder.id} onClick={() => setCurrentFolderId(folder.id)} onContextMenu={(event) => { event.preventDefault(); if (!folder.smart) { setContextMenu(null); setFolderContextMenu({ folder, x: event.clientX, y: event.clientY }); } }}><span aria-hidden="true">{folder.smart ? "◇" : "📁"}</span><strong>{folder.name} <b className="folder-count">{folderMediaCount(folder)}</b></strong><small>{folder.smart ? "No folder assignments" : "Open folder"}</small></button>)}</div>}
        {matchingFolderResults.length > 0 && <div className="folder-grid search-folder-results">{matchingFolderResults.map((folder) => <button className="folder-card search-result" key={`search-${folder.id}`} onClick={() => setCurrentFolderId(folder.id)}><span aria-hidden="true">📁</span><strong>{folder.name} <b className="folder-count">{folderMediaCount(folder)}</b></strong><small>{folder.path}</small></button>)}</div>}
        {filtered.length === 0 ? displayedFolders.length === 0 && matchingFolderResults.length === 0 && <div className="empty"><span>*</span><h3>{items.length ? "Nothing in this view" : "Your shelf is empty"}</h3><p>{items.length ? "Try a broader search or another folder." : "Add a remote image or import a local file to begin."}</p></div> :
          <div className="media-grid">{filtered.map((item) => <article className={`card ${brokenPreviews.has(item.id) || !itemPreview(item) ? "no-preview" : ""}`} key={item.id} onContextMenu={(event) => { event.preventDefault(); setFolderContextMenu(null); setContextMenu({ item, x: event.clientX, y: event.clientY }); }}>
            <button className="preview" onClick={() => activate(item)} aria-label={itemLink(item) ? `Copy link for ${item.title}` : `Copy file for ${item.title}`}>
              {brokenPreviews.has(item.id) || !itemPreview(item) ? <span className="preview-placeholder" aria-hidden="true">No preview</span> : <img src={itemPreview(item)} alt="" loading="lazy" onError={() => setBrokenPreviews((current) => new Set(current).add(item.id))} />}<span className={`source-badge ${hasLocalFile(item) ? "local" : ""}`}>{itemLink(item) ? "Copy link" : hasLocalFile(item) ? "Copy file" : "Unavailable"}</span>
            </button>
            <div className="card-body">{currentFolderId !== null && <label className="media-select"><input type="checkbox" checked={selectedMediaIds.has(item.id)} onChange={() => toggleMediaSelection(item.id)} />Select</label>}<h3 title={item.title}>{item.title}</h3><div className="chips">{item.folderNames.map((name) => <span className="folder" key={`f-${name}`}>{name}</span>)}</div><div className="label-block"><div><strong>Tags</strong><button disabled={item.tagNames.length === 0} onClick={() => copyLabels("tags", item.tagNames)}>Copy all</button></div><p>{item.tagNames.length ? item.tagNames.map((name) => `#${name}`).join(", ") : "No tags"}</p></div><div className="label-block"><div><strong>Aliases</strong><button disabled={item.aliasNames.length === 0} onClick={() => copyLabels("aliases", item.aliasNames)}>Copy all</button></div><p>{item.aliasNames.length ? item.aliasNames.join(", ") : "No aliases"}</p></div>
              {item.sourceKind === "local_file" && <button className="text-button" onClick={() => invoke("reveal_local_media", { id: item.id }).catch((reason) => setError(String(reason)))}>Reveal in folder</button>}
              <div className="card-actions"><button onClick={() => beginEdit(item)}>Edit</button><button className="danger-link" onClick={() => setDeleteItem(item)}>Remove</button></div>
            </div>
          </article>)}</div>}
      </section>
    </div>
    {(notice || error) && <p className={error ? "toast error" : "toast"} role="status">{error || notice}</p>}
    {exportOpen && <div className="modal-backdrop"><section className="modal" role="dialog" aria-modal="true" aria-labelledby="export-title"><h2 id="export-title">Export</h2><p className="privacy-warning">Exports may contain remote URLs, notes, tags, aliases, folder names, and, with a full backup, copies of local files.</p><p>Folder exports include descendant folders recursively.</p><label>Export scope<select value={exportScope} onChange={(event) => setExportScope(event.target.value as ExportScope)}><option value="all">Entire Library</option><option value="current">Current Folder{currentFolder ? `: ${currentFolder.path}` : " (All media)"}</option></select></label><div className="stacked-actions"><button className="primary" disabled={busy} onClick={() => exportFile("library")}>Export Library</button><button className="secondary" disabled={busy} onClick={() => exportFile("aliases")}>Export Aliases</button><button className="secondary" onClick={() => setExportOpen(false)}>Cancel</button></div></section></div>}
    {settingsOpen && <div className="modal-backdrop"><section className="modal settings-modal" role="dialog" aria-modal="true" aria-labelledby="settings-title"><h2 id="settings-title">Settings</h2><section className="settings-group"><h3>Export destination</h3><label>Export location<input readOnly value={exportSettings?.directory ?? "Default Downloads behavior"} /></label><div className="keybind-actions"><button onClick={chooseExportDirectory}>Choose...</button><button disabled={!exportSettings?.directory} onClick={clearExportDirectory}>Clear</button><span>{exportSettings?.folderName ?? "Default"}</span></div>{exportSettings?.directory && !exportSettings.exists && <p className="keybind-warning">This folder no longer exists. Choose a new export location.</p>}</section><section className="settings-group"><h3>Keybinds</h3><p>Optional global shortcuts work only while the app is running.</p><label>Show/focus app<input readOnly value={showFocusDraft} onKeyDown={(event) => recordKeybind(event, "show")} placeholder="Press a shortcut" /></label><div className="keybind-actions"><button disabled={keybindBusy !== null || !showFocusDraft.trim()} onClick={() => saveKeybind("show")}>Apply</button><button disabled={keybindBusy !== null || !keybinds?.showFocusKeybind} onClick={() => saveKeybind("show", true)}>Clear</button><span>{keybinds?.showFocusKeybind ? keybinds.showFocusActive ? "Active" : "Inactive" : "Unset"}</span></div><label>Toggle app<input readOnly value={toggleDraft} onKeyDown={(event) => recordKeybind(event, "toggle")} placeholder="Press a shortcut" /></label><div className="keybind-actions"><button disabled={keybindBusy !== null || !toggleDraft.trim()} onClick={() => saveKeybind("toggle")}>Apply</button><button disabled={keybindBusy !== null || !keybinds?.toggleVisibilityKeybind} onClick={() => saveKeybind("toggle", true)}>Clear</button><span>{keybinds?.toggleVisibilityKeybind ? keybinds.toggleVisibilityActive ? "Active" : "Inactive" : "Unset"}</span></div>{keybinds?.warnings.map((warning) => <p className="keybind-warning" key={warning}>{warning}</p>)}</section><section className="settings-group"><h3>Library cleanup</h3><button className="secondary" disabled={busy} onClick={inspectDuplicatePurge}>Purge duplicates</button></section><section className="settings-group"><h3>Preview storage</h3><p>Static PNG and JPG/JPEG records can use their managed original instead of an older generated thumbnail.</p><button className="secondary" disabled={busy} onClick={purgeStaticThumbnails}>Purge static image thumbnails</button></section><section className="settings-group danger-zone"><h3>App management</h3><button className="secondary" onClick={uninstallApp}>Uninstall app</button><button className="danger" onClick={() => setWipeOpen(true)}>Wipe all</button></section><div className="modal-actions"><button className="secondary" onClick={() => setSettingsOpen(false)}>Close</button></div></section></div>}
    {duplicatePreview && <div className="modal-backdrop nested-modal"><section className="modal" role="alertdialog" aria-modal="true" aria-labelledby="duplicates-title"><h2 id="duplicates-title">Purge duplicate folder items?</h2><p>Scanned {duplicatePreview.scopesScanned} folder scope{duplicatePreview.scopesScanned === 1 ? "" : "s"}. Found {duplicatePreview.duplicateGroups} duplicate group{duplicatePreview.duplicateGroups === 1 ? "" : "s"} and {duplicatePreview.membershipRemovals} duplicate item{duplicatePreview.membershipRemovals === 1 ? "" : "s"} to remove: {duplicatePreview.normalFolderRemovals} in normal folders and {duplicatePreview.uncategorizedRemovals} in Uncategorized. Duplicates in different folders will not be removed. Original media files and previews are never deleted.</p>{duplicatePreview.titleOnlyGroupsSkipped > 0 && <p>Possible title-only duplicates were not purged automatically: {duplicatePreview.titleOnlyGroupsSkipped}.</p>}{duplicatePreview.groupReasons.length > 0 && <div className="duplicate-reasons"><strong>Matched by</strong>{duplicatePreview.groupReasons.slice(0, 6).map((reason) => <span key={reason}>{reason}</span>)}</div>}<div className="modal-actions"><button className="secondary" disabled={busy} onClick={() => setDuplicatePreview(null)}>Cancel</button><button className="danger" disabled={busy} onClick={purgeDuplicates}>Purge duplicate folder items</button></div></section></div>}
    {wipeOpen && <div className="modal-backdrop nested-modal"><section className="modal" role="alertdialog" aria-modal="true" aria-labelledby="wipe-title"><h2 id="wipe-title">Permanently wipe library?</h2><p>This will wipe the local library database, folders, tags, aliases, settings, usage history, and generated previews. It will not delete your original media files. Export your library first if you want a backup.</p><label>Type <strong>Reset</strong> exactly<input value={wipeConfirmation} onChange={(event) => setWipeConfirmation(event.target.value)} /></label><div className="modal-actions"><button className="secondary" disabled={busy} onClick={() => { setWipeOpen(false); setWipeConfirmation(""); }}>Cancel</button><button className="danger" disabled={busy || wipeConfirmation !== "Reset"} onClick={wipeAll}>Permanently wipe library</button></div></section></div>}
    {importPreview && <div className="modal-backdrop" role="presentation"><section className="modal import-modal" role="dialog" aria-modal="true" aria-labelledby="import-title"><h2 id="import-title">Review import</h2><div className="import-preview"><strong>{importPreview.kind === "library" ? "Library backup" : "Alias file"}</strong><p>{importPreview.itemCount} total item{importPreview.itemCount === 1 ? "" : "s"} · {importPreview.localFileCount} local · {importPreview.remoteUrlCount} remote</p>{importPreview.kind === "aliases" ? <><p>{importPreview.aliasMatchCount} globally matched existing media before scope filtering.</p><p>{importPreview.aliasUnmatchedCount} unmatched and will be skipped{importPreview.aliasUnmatched.length ? `: ${importPreview.aliasUnmatched.slice(0, 5).join(", ")}` : ""}.</p><p>Alias imports match existing media by URL or file hash. They do not create media or change folder memberships.</p></> : <p>{importPreview.conflictCount} conflict{importPreview.conflictCount === 1 ? "" : "s"}{importPreview.conflicts.length ? `: ${importPreview.conflicts.slice(0, 3).join(", ")}` : ""}</p>}</div>{importPreview.kind === "library" && importPreview.conflictCount > 0 && <label>Conflicts<select value={conflictStrategy} onChange={(event) => setConflictStrategy(event.target.value)}><option value="skip">Skip existing items</option><option value="import_anyway">Import another copy</option></select></label>}<label>{importPreview.kind === "aliases" ? "Alias match scope" : "Import destination"}<select value={importDestination} onChange={(event) => setImportDestination(event.target.value as ImportDestination)}><option value="current">{importPreview.kind === "aliases" ? currentFolder ? `Current view only: ${currentFolder.path}` : "All media" : currentFolder?.smart ? "Current view: Uncategorized (preserve imported folders; no folder added)" : currentFolder ? `Current folder: ${currentFolder.path} (add this folder)` : "Current view: All media (preserve imported folders; no folder added)"}</option><option value="root">{importPreview.kind === "aliases" ? "All media" : "Preserve imported folders / no added folder"}</option><option value="existing">{importPreview.kind === "aliases" ? "Choose existing folder scope" : "Choose existing folder to add"}</option>{importPreview.kind !== "aliases" && <option value="create">Create and add a new folder</option>}</select></label>{importDestination === "existing" && <label>Existing folder<select value={importFolderId} onChange={(event) => setImportFolderId(event.target.value)}><option value="">Choose folder...</option>{folders.map((folder) => <option value={folder.id} key={folder.id}>{folder.path}</option>)}</select></label>}{importDestination === "create" && importPreview.kind !== "aliases" && <label>New folder name<input value={importNewFolder} onChange={(event) => setImportNewFolder(event.target.value)} placeholder={currentFolder && !currentFolder.smart ? `Inside ${currentFolder.name}` : "At library root"} /></label>}<div className="modal-actions"><button className="secondary" disabled={busy} onClick={closeImport}>Cancel</button><button className="primary" disabled={busy} onClick={applyTransfer}>{busy ? "Importing..." : "Apply import"}</button></div></section></div>}
    {editState && <div className="modal-backdrop" role="presentation"><section className="modal" role="dialog" aria-modal="true" aria-labelledby="edit-title"><h2 id="edit-title">Edit media</h2><label>Title<input value={editState.title} onChange={(event) => setEditState({...editState, title: event.target.value})} /></label><label>Notes<textarea rows={3} value={editState.notes} onChange={(event) => setEditState({...editState, notes: event.target.value})} /></label><label>External URL<input type="url" value={editState.externalUrl} onChange={(event) => setEditState({...editState, externalUrl: event.target.value})} placeholder="https://..." /></label><label>Tags<input value={editState.tags} onChange={(event) => setEditState({...editState, tags: event.target.value})} /></label><label>Aliases<input value={editState.aliases} onChange={(event) => setEditState({...editState, aliases: event.target.value})} /></label><div className="preview-actions"><button className="secondary" type="button" onClick={() => uploadPreview(editState.item)}>{editState.item.hasManualPreview || previews[editState.item.id] ? "Replace preview" : "Upload preview"}</button><button className="secondary" type="button" disabled={!editState.item.hasManualPreview} onClick={() => clearManualPreview(editState.item)}>Clear uploaded preview</button></div><fieldset className="folder-picker"><legend>Folders</legend>{folders.map((folder) => <label key={folder.id}><input type="checkbox" checked={editState.folderIds.includes(folder.id)} onChange={() => setEditState({...editState, folderIds: editState.folderIds.includes(folder.id) ? editState.folderIds.filter((id) => id !== folder.id) : [...editState.folderIds, folder.id]})} /><span>{folder.path}</span></label>)}</fieldset><div className="modal-actions"><button className="secondary" onClick={() => setEditState(null)}>Cancel</button><button className="primary" disabled={busy || !editState.title.trim()} onClick={saveEdit}>Save changes</button></div></section></div>}
    {deleteItem && <div className="modal-backdrop" role="presentation"><section className="modal" role="dialog" aria-modal="true" aria-labelledby="remove-title"><h2 id="remove-title">Remove from library?</h2><p>“{deleteItem.title}” will be removed from the library. Managed media files will not be permanently deleted.{deleteItem.hostedUrl ? " Its hosted object will NOT be deleted and may remain publicly accessible." : ""}</p><div className="modal-actions"><button className="secondary" onClick={() => setDeleteItem(null)}>Cancel</button><button className="danger" onClick={confirmDeleteMedia}>Remove from library</button></div></section></div>}
    {folderDelete && <div className="modal-backdrop" role="presentation"><section className="modal" role="dialog" aria-modal="true" aria-labelledby="folder-delete-title"><h2 id="folder-delete-title">Delete “{folderDelete.folder.path}”?</h2><p>This folder contains {folderDelete.childFolderCount} child folder(s) and {folderDelete.mediaCount} media item(s). Media records will not be deleted.</p><div className="stacked-actions"><button className="danger" onClick={() => confirmFolderDelete("delete_subtree")}>Delete this folder and child folders</button><button className="secondary" onClick={() => confirmFolderDelete("reparent")}>Delete only this folder and move children/media to parent</button><button className="secondary" onClick={() => setFolderDelete(null)}>Cancel</button></div></section></div>}
    {contextMenu && <><button className="context-dismiss" aria-label="Close media menu" onClick={() => setContextMenu(null)} /><div className="context-menu" role="menu" style={{ left: contextMenu.x, top: contextMenu.y }}>
      <button role="menuitem" disabled={!itemLink(contextMenu.item)} onClick={() => { void copyLink(contextMenu.item); setContextMenu(null); }}>Copy link</button>
      <button role="menuitem" disabled={!hasLocalFile(contextMenu.item)} onClick={() => { void copyFile(contextMenu.item); setContextMenu(null); }}>Copy file</button>
      <button role="menuitem" onClick={() => { void uploadPreview(contextMenu.item); }}>Upload preview</button>
      <button role="menuitem" disabled={!contextMenu.item.hasManualPreview} onClick={() => { void clearManualPreview(contextMenu.item); setContextMenu(null); }}>Clear uploaded preview</button>
      <button role="menuitem" onClick={() => { setDeleteItem(contextMenu.item); setContextMenu(null); }}>Remove</button>
      <button role="menuitem" onClick={() => { beginEdit(contextMenu.item); setContextMenu(null); }}>Edit tags</button>
    </div></>}
    {folderContextMenu && <><button className="context-dismiss" aria-label="Close folder menu" onClick={() => setFolderContextMenu(null)} /><div className="context-menu" role="menu" style={{ left: folderContextMenu.x, top: folderContextMenu.y }}><button role="menuitem" onClick={() => { setCurrentFolderId(folderContextMenu.folder.id); setFolderContextMenu(null); }}>Open</button><button role="menuitem" onClick={() => { renameExistingFolder(folderContextMenu.folder); setFolderContextMenu(null); }}>Rename</button><button role="menuitem" onClick={() => { inspectFolderDelete(folderContextMenu.folder); setFolderContextMenu(null); }}>Remove</button></div></>}
    {(showReturnTop || displayedFolders.length + matchingFolderResults.length + filtered.length > 30) && <button className="return-top" onClick={() => window.scrollTo({ top: 0, behavior: "smooth" })}>Return to top</button>}
  </main>;
}

function isFolderDescendantOf(folder: FolderRecord, ancestorId: number, folders: FolderRecord[]) {
  let cursor: FolderRecord | undefined = folder;
  while (cursor && cursor.parentId !== null) {
    if (cursor.parentId === ancestorId) return true;
    cursor = folders.find((value) => value.id === cursor!.parentId);
  }
  return false;
}

function fileType(file: File) {
  const extension = file.name.split(".").pop()?.toLowerCase();
  if (!extension || !["gif", "png", "jpg", "jpeg", "webp"].includes(extension)) throw new Error("Unsupported file type.");
  return extension;
}

function isAnimatedPng(bytes: Uint8Array) {
  for (let index = 8; index + 4 <= bytes.length; index += 1) {
    if (bytes[index] === 0x61 && bytes[index + 1] === 0x63 && bytes[index + 2] === 0x54 && bytes[index + 3] === 0x4c) return true;
  }
  return false;
}

async function makeThumbnail(file: File) {
  const bitmap = await createImageBitmap(file);
  const scale = Math.min(1, 360 / bitmap.width, 240 / bitmap.height);
  const canvas = document.createElement("canvas"); canvas.width = Math.max(1, Math.round(bitmap.width * scale)); canvas.height = Math.max(1, Math.round(bitmap.height * scale));
  canvas.getContext("2d")!.drawImage(bitmap, 0, 0, canvas.width, canvas.height); bitmap.close();
  const blob = await new Promise<Blob>((resolve, reject) => canvas.toBlob((value) => value ? resolve(value) : reject(new Error("Could not create thumbnail.")), "image/webp", 0.82));
  return new Uint8Array(await blob.arrayBuffer());
}

function downloadJson(payload: string, filename: string) {
  const url = URL.createObjectURL(new Blob([payload], { type: "application/json" }));
  const link = document.createElement("a"); link.href = url; link.download = filename; link.click();
  setTimeout(() => URL.revokeObjectURL(url), 0);
}

function exportFilename(kind: "library" | "aliases", folderName: string | null, date: string) {
  const prefix = kind === "aliases" ? "gif-alias-export" : "gif-library-export";
  const folder = folderName ? sanitizeFilename(folderName) : "";
  return `${prefix}${folder ? `-${folder}` : ""}-${date}.json`;
}

function sanitizeFilename(value: string) {
  return value.replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").trim().replace(/[. ]+$/g, "");
}
