import { useQuery } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";

import { setGlobalInstructions, setProjectInstructions, type Project } from "../api";
import { m } from "../paraglide/messages.js";
import { getGlobalInstructionsQuery, getProjectInstructionsQuery } from "../queries/settings";
import { setScopedQueryData } from "../queries/client";
import { Button, LoadingRow, Spinner } from "./ui";

const CARD_CLASS_NAME = "flex flex-col gap-3 rounded-lg border border-border bg-background px-4.5 py-4 text-sm";
const TEXTAREA_CLASS_NAME =
  "min-h-72 w-full resize-y rounded-md border border-border bg-background px-2.5 py-2 font-mono text-sm leading-relaxed text-text outline-none placeholder:text-muted focus:border-text";

/** A textarea with Save: the text is only sent when it differs from what is stored. */
function InstructionsEditor({
  stored,
  placeholder,
  onSave,
  extraActions,
}: {
  stored: string;
  placeholder: string;
  onSave: (text: string) => Promise<unknown>;
  extraActions?: React.ReactNode;
}) {
  const [draft, setDraft] = useState(stored);
  // When the stored text changes underneath (the file was edited elsewhere),
  // follow it, unless that would throw away what is being typed.
  const lastStored = useRef(stored);
  useEffect(() => {
    setDraft((current) => (current === lastStored.current ? stored : current));
    lastStored.current = stored;
  }, [stored]);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const dirty = draft !== stored;

  const save = (event: React.FormEvent) => {
    event.preventDefault();
    if (saving || !dirty) return;
    setSaving(true);
    setError(null);
    onSave(draft)
      .catch((err) => setError(err instanceof Error ? err.message : String(err)))
      .finally(() => setSaving(false));
  };

  return (
    <form className="flex flex-col gap-3" onSubmit={save}>
      <textarea
        className={TEXTAREA_CLASS_NAME}
        value={draft}
        onChange={(event) => setDraft(event.target.value)}
        placeholder={placeholder}
        spellCheck
        dir="auto"
      />
      <div className="flex items-center gap-2">
        <Button type="submit" disabled={saving || !dirty}>
          {saving ? m.common_saving() : m.common_save()}
        </Button>
        {extraActions}
      </div>
      {error && <div className="text-accent-red whitespace-pre-wrap">{error}</div>}
    </form>
  );
}

/** The Instructions tab on the Projects page: rules every agent follows in every project. */
export function GlobalInstructionsSettings() {
  const options = getGlobalInstructionsQuery();
  const query = useQuery(options);
  const current = query.data;

  if (!current) {
    return query.error ? (
      <div className="text-accent-red">{query.error.message}</div>
    ) : (
      <LoadingRow><Spinner /> {m.settings_page_loading()}</LoadingRow>
    );
  }

  const apply = (next: typeof current) => setScopedQueryData(options.queryKey, next);
  return (
    <div className={CARD_CLASS_NAME}>
      <p className="m-0 text-subtext">{m.instructions_global_description()}</p>
      {current.isDefault && <p className="m-0 text-subtext">{m.instructions_using_default()}</p>}
      <InstructionsEditor
        key={`${current.isDefault}:${current.text}`}
        stored={current.text}
        placeholder={m.instructions_global_placeholder()}
        // Text identical to the built-in default is stored as "unset", so a later
        // improvement to the default still reaches anyone who never changed it.
        onSave={(text) => setGlobalInstructions(text === current.defaultText ? null : text).then(apply)}
        extraActions={
          !current.isDefault && (
            <Button type="button" onClick={() => void setGlobalInstructions(null).then(apply)}>
              {m.instructions_restore_default()}
            </Button>
          )
        }
      />
    </div>
  );
}

/** Settings → Instructions: rules for the open project only, added after the global ones. */
export function ProjectInstructionsSection({ project }: { project: Project }) {
  const options = getProjectInstructionsQuery(project.id);
  const query = useQuery(options);
  const current = query.data;

  return (
    <>
      <h2>{m.instructions_title()}</h2>
      <div className="mt-3">
        {!current ? (
          query.error ? (
            <div className="text-accent-red">{query.error.message}</div>
          ) : (
            <LoadingRow><Spinner /> {m.settings_page_loading()}</LoadingRow>
          )
        ) : (
          <div className={CARD_CLASS_NAME}>
            <p className="m-0 text-subtext">{m.instructions_project_description({ project: project.name })}</p>
            <InstructionsEditor
              stored={current.text}
              placeholder={m.instructions_project_placeholder()}
              onSave={(text) => setProjectInstructions(project.id, text).then((saved) => setScopedQueryData(options.queryKey, saved))}
            />
          </div>
        )}
      </div>
    </>
  );
}
