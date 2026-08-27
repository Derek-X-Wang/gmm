import { commandFailureFrom } from "./commandError";

export function CommandErrorNotice({
  error,
  heading,
  detail,
}: {
  error: unknown;
  heading?: string;
  detail?: string;
}) {
  if (error == null) return null;
  const failure = commandFailureFrom(error);

  if (heading || detail) {
    return (
      <div
        className="error"
        role="alert"
        data-command-failure-kind={failure.kind}
      >
        {heading ? <strong>{heading}</strong> : null}
        {detail ? <p>{detail}</p> : null}
        <ul>
          <li className="small">{failure.message}</li>
        </ul>
      </div>
    );
  }

  return (
    <p className="error" data-command-failure-kind={failure.kind}>
      {failure.message}
    </p>
  );
}
