import { screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";

import { renderWithQuery } from "./test/harness";

const auditLibrary = vi.fn();

vi.mock("./api", () => ({
  auditLibrary: (...args: unknown[]) => auditLibrary(...args),
}));

const { LibraryAuditWarning } = await import("./LibraryAuditWarning");

beforeEach(() => {
  auditLibrary.mockResolvedValue({
    game: "gimi",
    unreferenced: [
      {
        directoryName: "01FIRST",
        path: "C:\\Users\\me\\AppData\\Roaming\\GMM\\library\\gimi\\01FIRST",
        sizeBytes: 400 * 1024 * 1024,
      },
      {
        directoryName: "01SECOND",
        path: "D:\\GMM Library\\gimi\\01SECOND",
        sizeBytes: 12 * 1024 * 1024,
      },
    ],
    totalBytes: 412 * 1024 * 1024,
  });
});

it("shows unreferenced Library folders read-only in Settings", async () => {
  renderWithQuery(<LibraryAuditWarning game="gimi" />);

  const warning = await screen.findByRole("status");
  expect(warning).toHaveTextContent("2 unreferenced Library folders using 412 MB");
  expect(warning).toHaveTextContent(
    "C:\\Users\\me\\AppData\\Roaming\\GMM\\library\\gimi\\01FIRST",
  );
  expect(warning).toHaveTextContent("D:\\GMM Library\\gimi\\01SECOND");
  expect(screen.queryByRole("button")).not.toBeInTheDocument();
  expect(screen.queryByRole("link")).not.toBeInTheDocument();
});

it("renders no empty-state noise when the Library is fully referenced", async () => {
  auditLibrary.mockResolvedValue({
    game: "gimi",
    unreferenced: [],
    totalBytes: 0,
  });

  const { container } = renderWithQuery(<LibraryAuditWarning game="gimi" />);
  await waitFor(() => expect(auditLibrary).toHaveBeenCalledWith("gimi"));
  expect(container).toBeEmptyDOMElement();
});
