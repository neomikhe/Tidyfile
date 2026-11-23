type Waiting<In, Out> = { input: In; settle: (result: Out | null) => void };

export function latestOnly<In, Out>(work: (input: In) => Promise<Out>): (input: In) => Promise<Out | null> {
  let running = false;
  let waiting: Waiting<In, Out> | null = null;

  async function drain(input: In): Promise<Out> {
    running = true;
    try {
      return await work(input);
    } finally {
      running = false;
      const next = waiting;
      waiting = null;
      if (next !== null) void drain(next.input).then(next.settle, () => next.settle(null));
    }
  }

  return (input: In) => {
    if (!running) return drain(input);
    waiting?.settle(null);
    return new Promise((settle) => {
      waiting = { input, settle };
    });
  };
}
