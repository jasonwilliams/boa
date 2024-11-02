/**
 * Calculate the greatest common divisor of two numbers.
 * @param {number} a
 * @param {number} b
 * @param {function} callback A callback method to call with the result.
 * @returns {number|*} The greatest common divisor of {a} and {b}.
 * @throws {TypeError} If either {a} or {b} is not finite.
 */
export function gcd_callback(a, b, callback) {
  a = +a;
  b = +b;
  if (!Number.isFinite(a) || !Number.isFinite(b)) {
    throw new TypeError("Invalid input");
  }

  // Euclidean algorithm
  function inner_gcd(a, b) {
    while (b !== 0) {
      let t = b;
      b = a % b;
      a = t;
    }
    return a;
  }

  let result = inner_gcd(a, b);
  callback(result);
}