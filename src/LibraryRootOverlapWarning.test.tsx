import { render, screen } from "@testing-library/react";
import { expect, it } from "vitest";

import { LibraryRootOverlapWarning } from "./LibraryRootOverlapWarning";

it("names both paths and explains how to repair an unsafe root", () => {
  render(
    <LibraryRootOverlapWarning
      overlaps={[{
        game: "gimi",
        path: "C:\\Users\\user\\AppData\\Roaming\\GMM\\backups\\library",
        backups: "C:\\Users\\user\\AppData\\Roaming\\GMM\\backups",
      }]}
      modOverlaps={[]}
    />,
  );

  const warning = screen.getByRole("region", {
    name: /unsafe library paths/i,
  });
  expect(warning).toHaveTextContent(/GIMI configured root/i);
  expect(warning).toHaveTextContent(
    "C:\\Users\\user\\AppData\\Roaming\\GMM\\backups\\library",
  );
  expect(warning).toHaveTextContent(
    "C:\\Users\\user\\AppData\\Roaming\\GMM\\backups",
  );
  expect(warning).toHaveTextContent(/choose a different root/i);
  expect(warning).toHaveTextContent(/without reading or moving anything/i);
  expect(warning).toHaveTextContent(/audit, import, and relocation/i);
  expect(warning).toHaveTextContent(
    /enable, disable, and Junction reconciliation still use each Mod's recorded path/i,
  );
  expect(warning).not.toHaveTextContent(/GMM will not use these Library roots/i);
});

it("keeps naming Mods left in the backup tree after the configured root is repaired", () => {
  render(
    <LibraryRootOverlapWarning
      overlaps={[]}
      modOverlaps={[{
        modId: "01STRANDED",
        modName: "Stranded Mod",
        game: "gimi",
        path: "C:\\Users\\user\\AppData\\Roaming\\GMM\\backups\\legacy\\01STRANDED",
        backups: "C:\\Users\\user\\AppData\\Roaming\\GMM\\backups",
      }]}
    />,
  );

  const warning = screen.getByRole("region", {
    name: /unsafe library paths/i,
  });
  expect(warning).toHaveTextContent(/Mods still recorded inside the backup tree/i);
  expect(warning).toHaveTextContent(/Stranded Mod/i);
  expect(warning).toHaveTextContent(/01STRANDED/i);
  expect(warning).toHaveTextContent(
    "C:\\Users\\user\\AppData\\Roaming\\GMM\\backups\\legacy\\01STRANDED",
  );
  expect(warning).toHaveTextContent(/warning remains until no Mod record points there/i);
});

it("renders no empty-state noise for disjoint roots", () => {
  const { container } = render(
    <LibraryRootOverlapWarning overlaps={[]} modOverlaps={[]} />,
  );
  expect(container).toBeEmptyDOMElement();
});
