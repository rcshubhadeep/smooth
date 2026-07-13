import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  CheckCircle2,
  CircleAlert,
  FileUp,
  LoaderCircle,
  RefreshCw,
  Upload,
  X,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import "./ImportDocuments.css";

type ImportStatus =
  | "pending"
  | "processing"
  | "succeeded"
  | "failed"
  | "duplicate";

type ImportJob = {
  id: string;
  source_name: string;
  source_size: number;
  format: string;
  status: ImportStatus;
  attempts: number;
  max_attempts: number;
  note_id: string | null;
  warnings: string[];
  error: string | null;
  created_at: string;
  updated_at: string;
};

type ImportDocumentsProps = {
  onBankChanged: () => Promise<unknown>;
  onOpenNote: (noteId: string) => Promise<void>;
  onError: (message: unknown) => void;
  onSuccess: (message: string) => void;
};

const FILE_FILTERS = [
  {
    name: "Documents",
    extensions: [
      "pdf",
      "docx",
      "pptx",
      "xlsx",
      "xls",
      "html",
      "htm",
      "csv",
      "ipynb",
      "json",
      "xml",
      "txt",
      "md",
      "rst",
      "log",
      "toml",
      "yaml",
      "yml",
      "ini",
      "cfg",
      "py",
      "rs",
      "js",
      "jsx",
      "ts",
      "tsx",
      "c",
      "h",
      "cpp",
      "hpp",
      "go",
      "java",
      "rb",
      "swift",
      "sh",
      "sql",
      "css",
    ],
  },
];

export default function ImportDocuments({
  onBankChanged,
  onOpenNote,
  onError,
  onSuccess,
}: ImportDocumentsProps) {
  const [openPanel, setOpenPanel] = useState(false);
  const [jobs, setJobs] = useState<ImportJob[]>([]);
  const [choosing, setChoosing] = useState(false);
  const [retryingId, setRetryingId] = useState<string | null>(null);

  const loadJobs = useCallback(async () => {
    const next = await invoke<ImportJob[]>("list_import_jobs", { limit: 50 });
    setJobs(next);
    return next;
  }, []);

  useEffect(() => {
    void loadJobs().catch(() => undefined);
    const unlistenUpdated = listen<ImportJob>("import-job-updated", (event) => {
      setJobs((current) => upsertJob(current, event.payload));
      if (event.payload.status === "succeeded") {
        void onBankChanged();
      }
    });
    const unlistenCreated = listen<string>("import-note-created", () => {
      void onBankChanged();
    });
    return () => {
      void unlistenUpdated.then((unlisten) => unlisten());
      void unlistenCreated.then((unlisten) => unlisten());
    };
  }, [loadJobs, onBankChanged]);

  const hasActiveJobs = useMemo(
    () => jobs.some((job) => job.status === "pending" || job.status === "processing"),
    [jobs],
  );

  useEffect(() => {
    if (!hasActiveJobs) return;
    const timer = window.setInterval(() => {
      void loadJobs().catch(() => undefined);
    }, 1500);
    return () => window.clearInterval(timer);
  }, [hasActiveJobs, loadJobs]);

  async function chooseFiles() {
    setChoosing(true);
    try {
      const selection = await open({
        multiple: true,
        directory: false,
        filters: FILE_FILTERS,
      });
      if (!selection) return;
      const paths = Array.isArray(selection) ? selection : [selection];
      if (paths.length === 0) return;
      const queued = await invoke<ImportJob[]>("enqueue_imports", {
        request: { paths },
      });
      setJobs((current) => mergeJobs(current, queued));
      setOpenPanel(true);
      onSuccess(`${queued.length} ${queued.length === 1 ? "file" : "files"} queued for import`);
    } catch (error) {
      onError(error);
    } finally {
      setChoosing(false);
    }
  }

  async function retry(job: ImportJob) {
    setRetryingId(job.id);
    try {
      const updated = await invoke<ImportJob>("retry_import_job", { id: job.id });
      setJobs((current) => upsertJob(current, updated));
    } catch (error) {
      onError(error);
    } finally {
      setRetryingId(null);
    }
  }

  return (
    <>
      <button
        className={`icon-button${hasActiveJobs ? " active" : ""}`}
        type="button"
        onClick={() => setOpenPanel(true)}
        title="Import documents"
        aria-label="Import documents"
      >
        {hasActiveJobs ? <LoaderCircle className="spin" size={16} /> : <Upload size={16} />}
      </button>

      {openPanel ? (
        <div className="import-backdrop" role="presentation" onMouseDown={() => setOpenPanel(false)}>
          <section
            className="import-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="import-title"
            onMouseDown={(event) => event.stopPropagation()}
          >
            <header>
              <div>
                <h2 id="import-title">Import documents</h2>
                <p>Files become Markdown notes in Imported.</p>
              </div>
              <button type="button" onClick={() => setOpenPanel(false)} aria-label="Close">
                <X size={18} />
              </button>
            </header>

            <div className="import-job-list">
              {jobs.length === 0 ? (
                <div className="import-empty">
                  <FileUp size={26} />
                  <span>No imports yet</span>
                </div>
              ) : (
                jobs.map((job) => (
                  <article className={`import-job ${job.status}`} key={job.id}>
                    <JobIcon status={job.status} />
                    <div className="import-job-copy">
                      <strong title={job.source_name}>{job.source_name}</strong>
                      <small>
                        {statusLabel(job.status)} · {formatBytes(job.source_size)}
                      </small>
                      {job.error ? <p>{job.error}</p> : null}
                      {job.warnings.map((warning) => (
                        <p className="warning" key={warning}>{warning}</p>
                      ))}
                    </div>
                    <div className="import-job-actions">
                      {job.note_id && (job.status === "succeeded" || job.status === "duplicate") ? (
                        <button type="button" onClick={() => void onOpenNote(job.note_id!)}>
                          Open
                        </button>
                      ) : null}
                      {job.status === "failed" || job.status === "duplicate" ? (
                        <button
                          type="button"
                          onClick={() => void retry(job)}
                          disabled={retryingId === job.id}
                          title={job.status === "duplicate" ? "Import another copy" : "Retry import"}
                        >
                          <RefreshCw className={retryingId === job.id ? "spin" : ""} size={14} />
                          {job.status === "duplicate" ? "Import copy" : "Retry"}
                        </button>
                      ) : null}
                    </div>
                  </article>
                ))
              )}
            </div>

            <footer>
              <span>Up to 50 files per batch</span>
              <button className="primary" type="button" onClick={() => void chooseFiles()} disabled={choosing}>
                <Upload size={15} />
                {choosing ? "Choosing…" : "Choose files"}
              </button>
            </footer>
          </section>
        </div>
      ) : null}
    </>
  );
}

function JobIcon({ status }: { status: ImportStatus }) {
  if (status === "pending" || status === "processing") {
    return <LoaderCircle className="import-job-icon spin" size={18} />;
  }
  if (status === "succeeded") {
    return <CheckCircle2 className="import-job-icon" size={18} />;
  }
  return <CircleAlert className="import-job-icon" size={18} />;
}

function statusLabel(status: ImportStatus) {
  return {
    pending: "Queued",
    processing: "Converting",
    succeeded: "Imported",
    failed: "Failed",
    duplicate: "Already imported",
  }[status];
}

function formatBytes(bytes: number) {
  if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function upsertJob(current: ImportJob[], job: ImportJob) {
  return [job, ...current.filter((candidate) => candidate.id !== job.id)].sort(
    (first, second) => Number(second.created_at) - Number(first.created_at),
  );
}

function mergeJobs(current: ImportJob[], jobs: ImportJob[]) {
  return jobs.reduce(upsertJob, current);
}
