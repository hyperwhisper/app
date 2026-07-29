// Level as decibels below the maximum: 0 dB is the loudest a sample can be,
// normal speech sits near -26 dB, and anything under -46 dB is too quiet for
// the model to read.
export function db(level: number): string {
  if (level <= 0.00001) return "-inf";
  return (20 * Math.log10(level)).toFixed(0);
}
