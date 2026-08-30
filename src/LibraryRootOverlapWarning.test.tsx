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
    />,
  );

  const warning = screen.getByRole("region", {
    name: /unsafe library path configuration/i,
  });
  expect(warning).toHaveTextContent(/GIMI override/i);
  expect(warning).toHaveTextContent(
    "C:\\Users\\user\\AppData\\Roaming\\GMM\\backups\\library",
  );
  expect(warning).toHaveTextContent(
    "C:\\Users\\user\\AppData\\Roaming\\GMM\\backups",
  );
  expect(warning).toHaveTextContent(/choose a different root/i);
  expect(warning).toHaveTextContent(/without reading or moving anything/i);
});

it("renders no empty-state noise for disjoint roots", () => {
  const { container } = render(<LibraryRootOverlapWarning overlaps={[]} />);
  expect(container).toBeEmptyDOMElement();
});
