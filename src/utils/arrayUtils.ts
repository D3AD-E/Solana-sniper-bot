export const countOccurrences = <T>(arr: Array<T>, val: T): number => arr.reduce((a, v) => (v === val ? a + 1 : a), 0);
