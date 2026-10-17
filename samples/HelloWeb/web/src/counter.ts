/** A count the page shows and the buttons change. */
export class Counter {
  private value = 0

  constructor(readonly step: number) {}

  increment(by: number = this.step): number {
    this.value += by
    return this.value
  }

  get count(): number {
    return this.value
  }
}

export function formatCount(count: number, label: string): string {
  return `${label}: ${count}`
}
