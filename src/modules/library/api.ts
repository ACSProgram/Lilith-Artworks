import { invokeCommand } from "../../shared/tauri";
import type { CleanupReport } from "../../shared/fileCleanup";
import type {
  CreateArtworkRequest,
  LibrarySearchResult,
  LibraryTrashEntry,
  LibraryTree,
  MoveLibraryNodesRequest,
} from "./types";

export const libraryApi = {
  listTree: () => invokeCommand<LibraryTree>("list_library_tree"),
  search: (query: string) =>
    invokeCommand<LibrarySearchResult[]>("search_library", { query }),
  createGroup: (parentId: string | null, title: string) =>
    invokeCommand<LibraryTree>("create_library_group", { parentId, title }),
  createArtwork: (request: CreateArtworkRequest) =>
    invokeCommand<LibraryTree>("create_library_artwork", { request }),
  renameNode: (id: string, title: string) =>
    invokeCommand<LibraryTree>("rename_library_node", { id, title }),
  trashNodes: (ids: string[]) =>
    invokeCommand<LibraryTree>("trash_library_nodes", { ids }),
  listTrash: () => invokeCommand<LibraryTrashEntry[]>("list_library_trash"),
  restoreTrash: (id: string) =>
    invokeCommand<LibraryTree>("restore_library_trash", { id }),
  permanentlyDeleteTrash: (ids: string[]) =>
    invokeCommand<CleanupReport>("permanently_delete_library_trash", { ids }),
  emptyTrash: () => invokeCommand<CleanupReport>("empty_library_trash"),
  moveNodes: (request: MoveLibraryNodesRequest) =>
    invokeCommand<LibraryTree>("move_library_nodes", { request }),
};
