export type LibraryNodeKind = "group" | "artwork";

export interface PrimaryBranch {
  id: string;
  title: string;
  sourcePath: string;
}

export interface ArtworkSummary {
  description: string;
  branchCount: number;
  primaryBranch: PrimaryBranch | null;
}

export interface LibraryNode {
  id: string;
  parentId: string | null;
  kind: LibraryNodeKind;
  title: string;
  position: number;
  updatedMs: number;
  children: LibraryNode[];
  artwork: ArtworkSummary | null;
}

export interface LibraryTree {
  nodes: LibraryNode[];
  groupCount: number;
  artworkCount: number;
}

export interface LibrarySearchResult {
  id: string;
  kind: LibraryNodeKind;
  title: string;
  breadcrumb: string;
  ancestorIds: string[];
  sourcePath: string | null;
}

export interface LibraryTrashEntry {
  id: string;
  kind: LibraryNodeKind;
  title: string;
  deletedMs: number;
  descendantCount: number;
  artworkCount: number;
  originalParentTitle: string | null;
}

export interface CreateArtworkRequest {
  parentId: string | null;
  title: string;
  branchTitle: string;
  sourcePath: string;
}

export interface MoveLibraryNodesRequest {
  ids: string[];
  parentId: string | null;
  index: number;
}
