import { cleanup, screen, waitFor, within } from "@testing-library/react";
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

it("describes the delete confirmation and focuses its safe action", async () => {
  const user = userEvent.setup();
  renderWithQuery(<LibraryAuditWarning game="gimi" />);

  await user.click((await folder(FIRST)).getByRole("button", { name: /delete/i }));
  expect(deleteUnreferencedLibraryDir).not.toHaveBeenCalled();

  const confirm = await folder(FIRST);
  const confirmation = confirm.getByRole("group", { name: /confirm delete/i });
  expect(confirmation).toHaveAccessibleDescription(
    /Permanently delete 01FIRST.*400 MB.*cannot be undone/i,
  );
  expect(confirm.getByRole("button", { name: /cancel/i })).toHaveFocus();

  await user.click(confirm.getByRole("button", { name: /^delete$/i }));

  await waitFor(() =>
    expect(deleteUnreferencedLibraryDir).toHaveBeenCalledWith("gimi", FIRST),
  );
  expect(deleteUnreferencedLibraryDir).toHaveBeenCalledTimes(1);
  await waitFor(() => expect(auditLibrary).toHaveBeenCalledTimes(2));
});

it("states when a folder's size is unknown", async () => {
  const user = userEvent.setup();
  auditLibrary.mockResolvedValue({
    game: "gimi",
    unreferenced: [{ directoryName: "01UNKNOWN", path: FIRST, sizeBytes: null }],
    totalBytes: 0,
  });
  renderWithQuery(<LibraryAuditWarning game="gimi" />);

  await user.click((await folder(FIRST)).getByRole("button", { name: /delete/i }));

  expect(
    (await folder(FIRST)).getByRole("group", { name: /confirm delete/i }),
  ).toHaveAccessibleDescription(/Permanently delete 01UNKNOWN.*size is unknown/i);
});

it("returns focus to the triggering control when delete is cancelled", async () => {
  const user = userEvent.setup();
  renderWithQuery(<LibraryAuditWarning game="gimi" />);

  const deleteButton = (await folder(SECOND)).getByRole("button", { name: /delete/i });
  await user.click(deleteButton);
  await user.click((await folder(SECOND)).getByRole("button", { name: /cancel/i }));

  expect(deleteUnreferencedLibraryDir).not.toHaveBeenCalled();
  expect(screen.queryByRole("group", { name: /confirm delete/i })).not.toBeInTheDocument();
  expect(deleteButton).toHaveFocus();
});

it("announces a recovered folder and keeps focus when the panel would unmount", async () => {
  const user = userEvent.setup();
  auditLibrary
    .mockResolvedValueOnce({
      game: "gimi",
      unreferenced: [AUDIT_REPORT.unreferenced[0]],
      totalBytes: AUDIT_REPORT.unreferenced[0].sizeBytes,
    })
    .mockResolvedValue({ game: "gimi", unreferenced: [], totalBytes: 0 });
  recoverUnreferencedLibraryDir.mockResolvedValue({ id: "01FIRST", name: "Raiden" });
  renderWithQuery(<LibraryAuditWarning game="gimi" />);

  // jsdom cannot prove that assistive technology spoke. This instead guards
  // the required lifecycle: text changes inside a live region that already exists.
  const status = await screen.findByRole("status");
  expect(status).toBeEmptyDOMElement();
  await user.click((await folder(FIRST)).getByRole("button", { name: /recover/i }));
  await user.type((await folder(FIRST)).getByLabelText(/name/i), "Raiden");
  await user.click((await folder(FIRST)).getByRole("button", { name: /^recover$/i }));

  await waitFor(() => expect(status).toHaveTextContent("Recovered 01FIRST as Raiden"));
  expect(screen.getByRole("status")).toBe(status);
  await waitFor(() =>
    expect(
      screen.getByRole("region", { name: /unreferenced library folders/i }),
    ).toHaveFocus(),
  );
  expect(status).not.toHaveFocus();
  expect(document.activeElement).not.toBe(document.body);
});

it("announces the real freed size and keeps focus after the last delete", async () => {
  const user = userEvent.setup();
  auditLibrary
    .mockResolvedValueOnce({
      game: "gimi",
      unreferenced: [AUDIT_REPORT.unreferenced[0]],
      totalBytes: AUDIT_REPORT.unreferenced[0].sizeBytes,
    })
    .mockResolvedValue({ game: "gimi", unreferenced: [], totalBytes: 0 });
  renderWithQuery(<LibraryAuditWarning game="gimi" />);

  const status = await screen.findByRole("status");
  expect(status).toBeEmptyDOMElement();
  await user.click((await folder(FIRST)).getByRole("button", { name: /delete/i }));
  await user.click((await folder(FIRST)).getByRole("button", { name: /^delete$/i }));

  await waitFor(() => expect(status).toHaveTextContent("Deleted 01FIRST and freed 400 MB"));
  expect(screen.getByRole("status")).toBe(status);
  await waitFor(() =>
    expect(
      screen.getByRole("region", { name: /unreferenced library folders/i }),
    ).toHaveFocus(),
  );
  expect(status).not.toHaveFocus();
  expect(document.activeElement).not.toBe(document.body);
});

it("one confirmed delete removes only the selected folder", async () => {
  const user = userEvent.setup();
  auditLibrary
    .mockResolvedValueOnce(AUDIT_REPORT)
    .mockResolvedValue({
      game: "gimi",
      unreferenced: [AUDIT_REPORT.unreferenced[1]],
      totalBytes: AUDIT_REPORT.unreferenced[1].sizeBytes,
    });
  renderWithQuery(<LibraryAuditWarning game="gimi" />);

  await user.click((await folder(FIRST)).getByRole("button", { name: /delete/i }));
  await user.click((await folder(FIRST)).getByRole("button", { name: /^delete$/i }));

  await waitFor(() =>
    expect(deleteUnreferencedLibraryDir.mock.calls).toEqual([["gimi", FIRST]]),
  );
  await waitFor(() => expect(screen.queryByText(FIRST)).not.toBeInTheDocument());
  expect(screen.getByText(SECOND)).toBeInTheDocument();
});

it("no control exposed across the panel can delete two folders in one confirmation", async () => {
  /**
   * Exercise every button initially present anywhere in the panel, then every
   * new button one click reveals. This catches an immediate bulk control and a
   * one-step confirmation regardless of its label or which row/panel section
   * owns it. It deliberately does not claim to crawl non-button controls or a
   * destructive flow that requires more than one confirmation step.
   */
  async function renderPanel() {
    const user = userEvent.setup();
    const view = renderWithQuery(<LibraryAuditWarning game="gimi" />);
    const panel = await screen.findByRole("region", {
      name: /unreferenced library folders/i,
    });
    const initialButtons = within(panel).getAllByRole("button");
    return { user, view, panel, initialButtons };
  }

  function resetCalls() {
    auditLibrary.mockClear();
    revealUnreferencedLibraryDir.mockClear();
    recoverUnreferencedLibraryDir.mockClear();
    deleteUnreferencedLibraryDir.mockClear();
  }

  const discovery = await renderPanel();
  const initialButtonCount = discovery.initialButtons.length;
  discovery.view.unmount();
  discovery.view.client.clear();
  cleanup();

  for (let first = 0; first < initialButtonCount; first += 1) {
    resetCalls();
    const path = await renderPanel();
    await path.user.click(path.initialButtons[first]);
    expect(
      deleteUnreferencedLibraryDir.mock.calls.length,
      `initial panel button ${first} deleted more than one folder`,
    ).toBeLessThanOrEqual(1);

    const revealedButtons = within(path.panel)
      .getAllByRole("button")
      .filter((button) => !path.initialButtons.includes(button));
    const revealedButtonCount = revealedButtons.length;
    path.view.unmount();
    path.view.client.clear();
    cleanup();

    for (let confirmation = 0; confirmation < revealedButtonCount; confirmation += 1) {
      resetCalls();
      const confirmedPath = await renderPanel();
      await confirmedPath.user.click(confirmedPath.initialButtons[first]);
      const confirmationButtons = within(confirmedPath.panel)
        .getAllByRole("button")
        .filter((button) => !confirmedPath.initialButtons.includes(button));
      await confirmedPath.user.click(confirmationButtons[confirmation]);

      expect(
        deleteUnreferencedLibraryDir.mock.calls.length,
        `panel button ${first}, confirmation button ${confirmation} deleted more than one folder`,
      ).toBeLessThanOrEqual(1);
      confirmedPath.view.unmount();
      confirmedPath.view.client.clear();
      cleanup();
    }
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

  const alerts = await screen.findAllByRole("alert");
  expect(alerts.find((alert) => /a Mod now references it/i.test(alert.textContent ?? "")))
    .toHaveTextContent(/a Mod now references it/i);
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

  const notice = await screen.findByRole("status");
  expect(notice).toBeEmptyDOMElement();
  await user.click((await folder(FIRST)).getByRole("button", { name: /delete/i }));
  await user.click((await folder(FIRST)).getByRole("button", { name: /^delete$/i }));

  await waitFor(() => expect(auditLibrary).toHaveBeenCalledTimes(2));
  expect(screen.getByRole("status")).toBe(notice);
  expect(notice).toHaveTextContent(/GMM will retry during a later startup/i);
  expect(notice).toHaveTextContent(quarantine);
  expect(notice).toHaveTextContent(/could not reclaim its disk space now/i);
  expect(notice).toHaveTextContent(/can still verify that directory at its reserved name/i);
  expect(notice).not.toHaveTextContent(/freed/i);
  await waitFor(() =>
    expect(
      screen.getByRole("region", { name: /unreferenced library folders/i }),
    ).toHaveFocus(),
  );
  expect(notice).not.toHaveFocus();
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

  const notice = await screen.findByRole("alert");
  expect(notice).toBeEmptyDOMElement();
  await user.click((await folder(FIRST)).getByRole("button", { name: /delete/i }));
  await user.click((await folder(FIRST)).getByRole("button", { name: /^delete$/i }));

  await waitFor(() => expect(auditLibrary).toHaveBeenCalledTimes(2));
  expect(screen.getByRole("alert")).toBe(notice);
  expect(notice).toHaveTextContent(FIRST);
  expect(notice).toHaveTextContent(/could not confirm whether its disk space was reclaimed/i);
  expect(notice).toHaveTextContent(/does not know whether any of that folder's bytes remain/i);
  expect(notice).toHaveTextContent(/verify the original directory at its reserved name/i);
  expect(notice).not.toHaveTextContent(quarantine);
  expect(notice).not.toHaveTextContent(/freed/i);
  await waitFor(() =>
    expect(
      screen.getByRole("region", { name: /unreferenced library folders/i }),
    ).toHaveFocus(),
  );
  expect(notice).not.toHaveFocus();
  expect(document.activeElement).not.toBe(document.body);
});
