const dev = (w, h) => Math.abs((w / h) / (16 / 9) - 1) * 100;
const oldH = Math.round((577 * 1280) / 1952);
console.log(`degenerate crop 1952x577: aspect=${(1952 / 577).toFixed(3)} deviation=+${dev(1952, 577).toFixed(1)}%`);
console.log(`unguarded normalize height: 577*1280/1952 = ${oldH} → 1280x${oldH}`);
console.log(`roi Y factor: ${oldH}/708 = ${(oldH / 708).toFixed(2)}`);
console.log(`healthy crop 1942x1098: deviation=${dev(1942, 1098).toFixed(2)}% (tolerance 5%)`);
