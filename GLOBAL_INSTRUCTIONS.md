## Experiment documentation

Keep two plain-language files in the project's artifacts directory, and keep them current as the work moves: `EXPERIMENT_PLAN.md` (what we are trying to learn, the experiments planned and why, and what each result changed about the plan) and `EXPERIMENT_LOG.md` (a prose diary of what was run and found). They complement the structured experiment and run records; link to those records where useful instead of copying them.

After every substantial experiment, add an entry to `EXPERIMENT_LOG.md` that says:

- exactly what was run (code, configuration, data, compute) and the question it answers;
- one or two representative inputs and the outputs the system produced for them, where that helps;
- the metrics used and why they fit;
- what was found, in plain words, and what it changes about the plan.

Use visuals (plots, tables, diagrams) wherever they show something faster than prose. Describe methods and findings slowly and simply, as if to a smart colleague from another field.

## Long-running and multi-job work

A multi-day experiment or a sweep of many jobs must not flood the chat.

- When a single job in a larger experiment finishes, answer in one or two sentences: which job, the outcome, and the headline metric. Do not narrate your reasoning or repeat earlier reports.
- Put the supporting analysis and context in `EXPERIMENT_LOG.md` and, when you add it, mention that in the same one-liner.
- Write a full report in the chat only when the whole experiment finishes or a decision needs the user.
- If an update adds nothing beyond what you already reported, say so in a single short sentence, or nothing at all when no reply is required.
- Failures are the exception: report a failed or stuck job promptly and clearly, with the likely cause and your next step.
