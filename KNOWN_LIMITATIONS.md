# Known Limitations

These limitations reflect the current local-library implementation.

## Filesystem rollback is best-effort

A process crash or power loss during the narrow interval between writing files and committing SQLite could still leave orphaned files. A future staging-directory or recovery mechanism would close that gap.

## Thumbnail fallback can be memory-heavy

When thumbnail generation fails, previewing falls back to reading the full original image. This is correct behavior for v0.1, but it may be less memory-efficient for large GIFs.

## Clipboard failure behavior is not automatically tested

The project currently has no frontend test harness, so clipboard failure handling is verified manually and behaviorally for now.

## Full backups can use substantial memory

Full JSON/base64 backups can use substantial memory for large libraries.

## Folder movement is not implemented

Folder rename and deletion are supported, but moving an existing folder to a different parent is not implemented.

## Unmatched alias imports are skipped

Alias-only imports match existing items by URL or file hash and never create placeholder media. Global, normal-folder, and Uncategorized scopes are supported; unmatched items and aliases that already exist are reported as skipped.

## Undo is intentionally limited

Undo intentionally stores only the three most recent remove-from-library, duplicate-purge, or bulk folder-organization actions in memory. Older entries are dropped, the stack does not survive restart, and it does not cover edits, imports, exports, folder deletion, clipboard actions, startup cleanup, external files, manual preview changes, or legacy hosted objects. This bounded behavior is a product decision, not a bug.

## WEBP preview optimization is conservative

Animated WEBP detection is not implemented and may remain unavailable, so all WEBP imports receive bounded generated previews even when static. APNG detection uses the standard `acTL` chunk: PNG with `acTL` is animated, and PNG without it is static. Unusual or malformed animation data without the standard chunk is treated as static. Redundant static thumbnails primarily affect older libraries; new static PNG/JPG/JPEG imports do not create them. Users can explicitly purge verified older static thumbnails from Settings.

## Manual preview cleanup is limited

Uploaded manual previews are stored in app-managed media storage and are protected from Purge static image thumbnails by using non-`*.thumb.webp` filenames. Replacing or clearing an uploaded preview best-effort removes the previous manual preview file. If a process crash or filesystem error happens during that narrow window, an orphaned preview file may remain until a future orphan-cleanup tool exists.

## Uninstall data removal is not customized

The app does not customize the Windows uninstaller to ask whether app data should be removed. Packaged builds can open Windows Installed Apps, while development builds have no uninstaller. Users should use Settings → Wipe all before uninstalling when they want to clear library state and generated previews. App data is stored under `%APPDATA%\app.giflibrary.desktop`; generated previews and managed originals are in its `media` directory. Users may manually remove that directory only when they intentionally want to delete the database and all managed copies.

## Wipe filesystem cleanup is best-effort

Wipe all transactionally clears SQLite first, then deletes only verified generated `*.thumb.webp` previews. Windows or another process may have a preview open or locked; failure to delete it is expected, reported as a best-effort warning, and is not critical. App-managed copied originals intentionally remain on disk as unreferenced files after the wipe.

## Frontend behavior is primarily tested manually

UI behavior beyond TypeScript compilation is primarily tested manually because the project does not currently have a frontend test harness. This is acceptable for the current private prototype.

This includes the import review modal, overlay-toast timing, folder navigation, sorting, global/current-view search behavior, manual preview upload flows, and tri-state bulk organization behavior.

## Duplicate cleanup intentionally avoids weak matches

Purge duplicates operates only inside exact normal-folder memberships and inside the derived Uncategorized scope as its own separate scope. Strong matching uses file hash, normalized URL, or managed storage filename. Medium matching uses normalized title plus same source kind/media type and overlapping aliases or tags. Pure title-only groups are reported as possible duplicates but are not purged automatically. Records without strong identity or safe title+metadata overlap are ignored. Cross-folder matches are deliberately not purged.

## Legacy hosted objects are unmanaged

Older builds may have created hosted URLs and remote objects. The app can still copy a stored legacy URL, but it no longer stores hosting credentials or manages, verifies, uploads, resets, or deletes hosted objects. Users must manage any older remote objects directly with their former provider.

## Failed imports can leave a newly created destination folder

When “Create new folder” is selected as an import destination, the folder is created before the transactional media import begins. If the subsequent import fails validation or cannot be committed, the empty folder remains and can be removed manually.
