-- SQLite is a compatibility-facing index and stores only the stable public history family.
-- Exact rollout generations remain gated by the canonical SessionMeta in the rollout itself.
UPDATE threads
SET history_mode = 'paginated'
WHERE history_mode = 'paginated_refs_v1';
