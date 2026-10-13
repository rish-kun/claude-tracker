-- +goose Up
INSERT INTO model_prices (model, input, cache_write_5m, cache_write_1h, cache_read, output,
                          fast_input, fast_output,
                          long_input, long_cache_write_5m, long_cache_write_1h, long_cache_read, long_output)
VALUES
  ('claude-fable-5-1',  10,   12.5,  20,   0.25, 50,  NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-fable-5',    10,   12.5,  20,   1.0,  50,  NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-opus-5-5',   4,    5,     8,    0.20, 20,  8,    40,   NULL, NULL, NULL, NULL, NULL),
  ('claude-opus-5',     5,    6.25,  10,   0.5,  25,  10,   50,   NULL, NULL, NULL, NULL, NULL),
  ('claude-opus-4-8',   5,    6.25,  10,   0.5,  25,  10,   50,   NULL, NULL, NULL, NULL, NULL),
  ('claude-opus-4-7',   5,    6.25,  10,   0.5,  25,  NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-opus-4-6',   5,    6.25,  10,   0.5,  25,  NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-opus-4-5',   5,    6.25,  10,   0.5,  25,  NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-opus-4-1',   15,   18.75, 30,   1.5,  75,  NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-opus-4',     15,   18.75, 30,   1.5,  75,  NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-sonnet-5-5', 2,    2.5,   4,    0.20, 10,  NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-sonnet-5',   2,    2.5,   4,    0.2,  10,  NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-sonnet-4-6', 3,    3.75,  6,    0.3,  15,  NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-sonnet-4-5', 3,    3.75,  6,    0.3,  15,  NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-sonnet-4',   3,    3.75,  6,    0.3,  15,  NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-haiku-5-5',  0.10, 0.125, 0.20, 0.01, 0.50, NULL, NULL, 0.5, 0.625, 1.0, 0.05, 2.5),
  ('claude-haiku-4-5',  1,    1.25,  2,    0.1,  5,   NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-3-5-haiku',  0.8,  1,     1.6,  0.08, 4,   NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-3-7-sonnet', 3,    3.75,  6,    0.3,  15,  NULL, NULL, NULL, NULL, NULL, NULL, NULL);

-- +goose Down
DELETE FROM model_prices;
