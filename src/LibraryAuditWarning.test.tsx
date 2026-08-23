import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";

import { renderWithQuery } from "./test/harness";

const auditLibrary = vi.fn();
const revealUnreferencedLibraryDir = vi.fn();
const recoverUnreferencedLibraryDir = vi.fn();
const deleteUnreferencedLibraryDir = vi.fn();

vi.mock("./api", () => ({
  auditLibrary: (...args: unknown[]) => auditLibrary(...args),
  revealUnreferencedLibraryDir: (...args: unknown[]) => revealUnreferencedLibraryDir(...args),
  recoverUnreferencedLibraryDir: (...args: unknown[]) => recoverUnreferencedLibraryDir(...args),
  deleteUnreferencedLibraryDir: (...args: unknown[]) => deleteUnreferencedLibraryDir(...args),
}));

const { LibraryAuditWarning } = await import("./LibraryAuditWarning");

const FIRST = "C:\\Users\\me\\AppData\\Roaming\\GMM\\library\\gimi\\01FIRST";
const SECOND = "D:\\GMM Library\\gimi\\01SECOND";
const AUDIT_REPORT = {
  game: "gimi",
  unreferenced: [
    { directoryName: "01FIRST", path: FIRST, sizeBytes: 400 * 1024 * 1024 },
    { directoryName: "01SECOND", path: SECOND, sizeBytes: 12 * 1024 * 1024 },
  ],
  totalBytes: 412 * 1024 * 1024,
};

beforeEach(() => {
  vi.clearAllMocks();
  auditLibrary.mockResolvedValue(AUDIT_REPORT);
  revealUnreferencedLibraryDir.mockResolvedValue(undefined);
  recoverUnreferencedLibraryDir.mockResolvedValue({ id: "01FIRST", name: "Raiden" });
  deleteUnreferencedLibraryDir.mockResolvedValue({
    directoryName: "01FIRST",
    path: FIRST,
    sizeBytes: 400 * 1024 * 1024,
    reclamation: { status: "reclaimed" },
  });
});

/** The list item for one unreferenced folder. */
async function folder(path: string) {
  const code = await screen.findByText(path);
  const item = code.closest("li");
  expect(item).not.toBeNull();
  return within(item as HTMLElement);
}

it("lists each unreferenced Library folder with its size", async () => {
  renderWithQuery(<LibraryAuditWarning game="gimi" />);

  const warning = await screen.findByRole("region", {
    name: /unreferenced library folders/i,
  });
  expect(warning).toHaveTextContent("2 unreferenced Library folders using 412 MB");
  expect(warning).toHaveTextContent(FIRST);
  expect(warning).toHaveTextContent(SECOND);
});

it("renders no empty-state noise when the Library is fully referenced", async () => {
  auditLibrary.mockResolvedValue({ game: "gimi", unreferenced: [], totalBytes: 0 });

  const { container } = renderWithQuery(<LibraryAuditWarning game="gimi" />);
  await waitFor(() => expect(auditLibrary).toHaveBeenCalledWith("gimi"));
  expect(container).toBeEmptyDOMElement();
});

it("inspects one folder by revealing it", async () => {
  const user = userEvent.setup();
  renderWithQuery(<LibraryAuditWarning game="gimi" />);

  await user.click((await folder(SECOND)).getByRole("button", { name: /inspect/i }));

  await waitFor(() =>
    expect(revealUnreferencedLibraryDir).toHaveBeenCalledWith("gimi", SECOND),
  );
  // Inspecting one folder must not reach for the other, and must not be a
  // read-only action that quietly changes something.
  expect(revealUnreferencedLibraryDir).toHaveBeenCalledTimes(1);
  expect(recoverUnreferencedLibraryDir).not.toHaveBeenCalled();
  expect(deleteUnreferencedLibraryDir).not.toHaveBeenCalled();
});

it("recovers a folder under a name the user types, and refreshes the report", async () => {
  const user = userEvent.setup();
  renderWithQuery(<LibraryAuditWarning game="gimi" />);

  await user.click((await folder(FIRST)).getByRole("button", { name: /recover/i }));

  const form = await folder(FIRST);
  const name = form.getByLabelText(/name/i);
  // Nothing can be recovered nameless — GMM must not invent one.
  expect(form.getByRole("button", { name: /^recover$/i })).toBeDisabled();

  await user.type(name, "Raiden Shogun Alt");
  await user.click(form.getByRole("button", { name: /^recover$/i }));

  await waitFor(() =>
    expect(recoverUnreferencedLibraryDir).toHaveBeenCalledWith(
      "gimi",
      FIRST,
      "Raiden Shogun Alt",
    ),
  );
  expect(deleteUnreferencedLibraryDir).not.toHaveBeenCalled();
  await waitFor(() => expect(auditLibrary).toHaveBeenCalledTimes(2));
});

it("deletes only after a confirmation naming the folder and its size", async () => {
  const user = userEvent.setup();
  renderWithQuery(<LibraryAuditWarning game="gimi" />);

  await user.click((await folder(FIRST)).getByRole("button", { name: /delete/i }));
  expect(deleteUnreferencedLibraryDir).not.toHaveBeenCalled();

  const confirm = await folder(FIRST);
  expect(confirm.getByRole("alertdialog")).toHaveTextContent("01FIRST");
  expect(confirm.getByRole("alertdialog")).toHaveTextContent("400 MB");

  await user.click(confirm.getByRole("button", { name: /^delete$/i }));

  await waitFor(() =>
    expect(deleteUnreferencedLibraryDir).toHaveBeenCalledWith("gimi", FIRST),
  );
  expect(deleteUnreferencedLibraryDir).toHaveBeenCalledTimes(1);
  await waitFor(() => expect(auditLibrary).toHaveBeenCalledTimes(2));
});

it("cancels a delete without touching the folder", async () => {
  const user = userEvent.setup();
  renderWithQuery(<LibraryAuditWarning game="gimi" />);

  await user.click((await folder(SECOND)).getByRole("button", { name: /delete/i }));
  await user.click((await folder(SECOND)).getByRole("button", { name: /cancel/i }));

  expect(deleteUnreferencedLibraryDir).not.toHaveBeenCalled();
  expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
});

it("offers no way to delete every folder at once", async () => {
  renderWithQuery(<LibraryAuditWarning game="gimi" />);
  await screen.findByRole("region", { name: /unreferenced library folders/i });

  for (const button of screen.getAllByRole("button")) {
    expect(button.textContent ?? "").not.toMatch(/all|every|\b2 folders\b/i);
  }
});

it("surfaces a refused action instead of silently doing nothing", async () => {
  const user = userEvent.setup();
  deleteUnreferencedLibraryDir.mockRejectedValue(
    "\"...\\\\01FIRST\" is not an unreferenced Library folder GMM can act on: a Mod now references it — refresh the report.",
  );
  renderWithQuery(<LibraryAuditWarning game="gimi" />);

  await user.click((await folder(FIRST)).getByRole("button", { name: /delete/i }));
  await user.click((await folder(FIRST)).getByRole("button", { name: /^delete$/i }));

  expect(await screen.findByRole("alert")).toHaveTextContent(
    /a Mod now references it/i,
  );
});

it("says deferred bytes remain at the reserved path and later startups will retry", async () => {
  const user = userEvent.setup();
  const quarantine = "C:\\Users\\me\\AppData\\Roaming\\GMM\\library\\gimi\\.gmm-delete-DEFERRED";
  auditLibrary
    .mockResolvedValueOnce(AUDIT_REPORT)
    .mockResolvedValue({ game: "gimi", unreferenced: [], totalBytes: 0 });
  deleteUnreferencedLibraryDir.mockResolvedValue({
    directoryName: "01FIRST",
    path: FIRST,
    sizeBytes: null,
    reclamation: { status: "deferred", path: quarantine },
  });
  renderWithQuery(<LibraryAuditWarning game="gimi" />);

  await user.click((await folder(FIRST)).getByRole("button", { name: /delete/i }));
  await user.click((await folder(FIRST)).getByRole("button", { name: /^delete$/i }));

  const notice = await screen.findByText(/GMM will retry during a later startup/i);
  await waitFor(() => expect(auditLibrary).toHaveBeenCalledTimes(2));
  expect(notice).toBeInTheDocument();
  expect(notice).toHaveTextContent(quarantine);
  expect(notice).toHaveTextContent(/could not reclaim its disk space now/i);
  expect(notice).toHaveTextContent(/remains at its reserved name/i);
});

it("announces ownership loss without presenting the reserved path as a cleanup target", async () => {
  const user = userEvent.setup();
  const quarantine = "C:\\Users\\me\\AppData\\Roaming\\GMM\\library\\gimi\\.gmm-delete-FAILED";
  auditLibrary
    .mockResolvedValueOnce(AUDIT_REPORT)
    .mockResolvedValue({ game: "gimi", unreferenced: [], totalBytes: 0 });
  deleteUnreferencedLibraryDir.mockResolvedValue({
    directoryName: "01FIRST",
    path: FIRST,
    sizeBytes: null,
    reclamation: { status: "ownershipLost" },
  });
  renderWithQuery(<LibraryAuditWarning game="gimi" />);

  await user.click((await folder(FIRST)).getByRole("button", { name: /delete/i }));
  await user.click((await folder(FIRST)).getByRole("button", { name: /^delete$/i }));

  const notice = await screen.findByRole("alert");
  await waitFor(() => expect(auditLibrary).toHaveBeenCalledTimes(2));
  expect(notice).toHaveTextContent(FIRST);
  expect(notice).toHaveTextContent(/could not confirm whether its disk space was reclaimed/i);
  expect(notice).toHaveTextContent(/no longer knows where that folder's bytes are/i);
  expect(notice).toHaveTextContent(/verify the original directory at its reserved name/i);
  expect(notice).not.toHaveTextContent(quarantine);
});
