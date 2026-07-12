import { invoke } from "@tauri-apps/api/core";
import { embed } from "@ternlight/base";

type EmbeddingJob = {
  id: number;
  note_id: string;
  chunks: Array<{ id: number; text: string }>;
};

export type SemanticSearchResult = {
  id: string;
  title: string;
  heading: string | null;
  excerpt: string;
  score: number;
  start_offset: number;
  end_offset: number;
};

export async function semanticSearch(query: string, limit = 50): Promise<SemanticSearchResult[]> {
  return invoke<SemanticSearchResult[]>("semantic_search_notes", {
    query,
    queryEmbedding: Array.from(embed(query)),
    limit,
  });
}

let indexing = false;
let stopped = false;

export function startSemanticIndexer(): () => void {
  stopped = false;
  if (!indexing) void indexLoop();
  return () => { stopped = true; };
}

async function indexLoop() {
  indexing = true;
  while (!stopped) {
    let job: EmbeddingJob | null = null;
    try {
      job = await invoke<EmbeddingJob | null>("claim_embedding_job");
      if (!job) {
        await delay(1500);
        continue;
      }
      const embeddings: Array<{ chunk_id: number; vector: number[] }> = [];
      for (const chunk of job.chunks) {
        embeddings.push({ chunk_id: chunk.id, vector: Array.from(embed(chunk.text)) });
        // Give rendering and input events a chance between inference calls.
        await delay(0);
      }
      await invoke("complete_embedding_job", { jobId: job.id, embeddings });
    } catch (error) {
      if (job) {
        await invoke("fail_embedding_job", {
          jobId: job.id,
          error: error instanceof Error ? error.message : String(error),
        }).catch(() => undefined);
      }
      await delay(2000);
    }
  }
  indexing = false;
}

function delay(milliseconds: number) {
  return new Promise((resolve) => window.setTimeout(resolve, milliseconds));
}
