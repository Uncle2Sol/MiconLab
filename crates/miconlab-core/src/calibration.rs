//! 校准算法模块。
//!
//! 提供零点校准、零点+满量程校准、最小二乘线性拟合与多项式拟合，
//! 并支持将拟合得到的系数应用于原始采样值。所有拟合均使用正规方程求解。

use serde::{Deserialize, Serialize};

/// 校准结果
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct CalibrationResult {
    /// 线性拟合系数
    pub coefficients: CalibrationCoefficients,
    /// 拟合优度 R²
    pub r_squared: f64,
    /// 残差（每个点的 y_actual - y_predicted）
    pub residuals: Vec<f64>,
    /// 校准点 (raw, standard)
    pub points: Vec<(f64, f64)>,
}

/// 校准系数（线性）
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct CalibrationCoefficients {
    /// 斜率
    pub slope: f64,
    /// 截距
    pub intercept: f64,
}

/// 零点校准：raw=0 时输出应为 0。
///
/// 给定零点采样的原始读数 `zero_raw`，构造系数使
/// `apply_linear(zero_raw, coeff) == 0`。
pub fn zero_calibration(zero_raw: f64) -> CalibrationCoefficients {
    CalibrationCoefficients {
        slope: 1.0,
        intercept: -zero_raw,
    }
}

/// 零点+满量程校准。
///
/// - `zero_raw`: 零点的原始读数
/// - `span_raw`: 满量程的原始读数
/// - `span_value`: 满量程对应的标准值
///
/// 计算结果使得：
/// - `apply_linear(zero_raw) == 0`
/// - `apply_linear(span_raw) == span_value`
pub fn zero_span_calibration(
    zero_raw: f64,
    span_raw: f64,
    span_value: f64,
) -> CalibrationCoefficients {
    let denom = span_raw - zero_raw;
    let slope = span_value / denom;
    let intercept = -zero_raw * slope;
    CalibrationCoefficients { slope, intercept }
}

/// 最小二乘线性拟合。
///
/// 拟合模型：`y = slope * x + intercept`
///
/// - `slope = (n*Σxy - Σx*Σy) / (n*Σx² - (Σx)²)`
/// - `intercept = (Σy - slope*Σx) / n`
/// - `R² = 1 - SS_res / SS_tot`
///
/// `points` 为 `(raw_value, standard_value)` 二元组切片。
/// 当点数小于 2 或分母为 0 时返回零系数与 R²=0。
pub fn linear_fit(points: &[(f64, f64)]) -> CalibrationResult {
    let n = points.len() as f64;
    if points.len() < 2 {
        return CalibrationResult {
            points: points.to_vec(),
            ..Default::default()
        };
    }

    let sum_x: f64 = points.iter().map(|(x, _)| *x).sum();
    let sum_y: f64 = points.iter().map(|(_, y)| *y).sum();
    let sum_xy: f64 = points.iter().map(|(x, y)| x * y).sum();
    let sum_x2: f64 = points.iter().map(|(x, _)| x * x).sum();

    let denom = n * sum_x2 - sum_x * sum_x;
    if denom.abs() < f64::EPSILON {
        // 所有 x 相同，无法拟合斜率
        return CalibrationResult {
            points: points.to_vec(),
            ..Default::default()
        };
    }

    let slope = (n * sum_xy - sum_x * sum_y) / denom;
    let intercept = (sum_y - slope * sum_x) / n;

    // 计算预测值、残差与 R²
    let y_mean = sum_y / n;
    let mut ss_tot = 0.0;
    let mut ss_res = 0.0;
    let mut residuals = Vec::with_capacity(points.len());
    for &(x, y) in points {
        let pred = slope * x + intercept;
        let resid = y - pred;
        residuals.push(resid);
        ss_res += resid * resid;
        ss_tot += (y - y_mean).powi(2);
    }

    let r_squared = if ss_tot.abs() < f64::EPSILON {
        // 所有 y 相同：若预测也完全相同则 R²=1，否则为 0
        if ss_res.abs() < f64::EPSILON {
            1.0
        } else {
            0.0
        }
    } else {
        1.0 - ss_res / ss_tot
    };

    CalibrationResult {
        coefficients: CalibrationCoefficients { slope, intercept },
        r_squared,
        residuals,
        points: points.to_vec(),
    }
}

/// 多项式拟合（最小二乘）。
///
/// - `degree`: 多项式阶数（1=线性, 2=二次, 3=三次）
/// - 返回系数向量 `[a0, a1, a2, ...]`，表示 `a0 + a1*x + a2*x² + ...`
///
/// 使用正规方程 `(Xᵀ·X)·a = Xᵀ·y`，通过高斯消元法求解。
/// 当点数不足以拟合或矩阵奇异时返回空向量。
///
/// # 特殊情况
/// - `degree = 0`：返回空向量（0 阶多项式即常数，无意义，请使用 [`zero_calibration`]
///   或直接计算均值）
/// - `points` 为空：返回空向量
/// - `points.len() < degree + 1`：点数不足，返回空向量
pub fn polynomial_fit(points: &[(f64, f64)], degree: usize) -> Vec<f64> {
    if points.is_empty() || degree == 0 {
        return Vec::new();
    }

    // 所需最少点数 = degree + 1
    if points.len() < degree + 1 {
        return Vec::new();
    }

    let m = degree + 1; // 系数个数

    // 构造设计矩阵 X 的转置 Xᵀ（维度 m×n），再计算 Xᵀ·X（m×m）与 Xᵀ·y（m）
    // Xᵀ·X 的元素 (i,j) = Σ x_k^(i+j)
    // Xᵀ·y 的元素 i = Σ x_k^i * y_k
    let mut xtx = vec![vec![0.0; m]; m]; // m×m
    let mut xty = vec![0.0; m]; // m

    for &(x, y) in points {
        // 预计算 x 的各次幂：x_pow[i] = x^i
        let mut x_pow = vec![1.0; 2 * m - 1];
        if 2 * m - 1 > 1 {
            for i in 1..2 * m - 1 {
                x_pow[i] = x_pow[i - 1] * x;
            }
        }
        for i in 0..m {
            xty[i] += x_pow[i] * y;
            for j in 0..m {
                xtx[i][j] += x_pow[i + j];
            }
        }
    }

    // 高斯消元法求解 xtx * a = xty（带部分主元选取）
    let mut a = xty;
    let mut mat = xtx;
    for col in 0..m {
        // 选主元：在 col..m 行中找绝对值最大的行
        let mut pivot = col;
        let mut max_val = mat[col][col].abs();
        for r in (col + 1)..m {
            if mat[r][col].abs() > max_val {
                max_val = mat[r][col].abs();
                pivot = r;
            }
        }
        if max_val < f64::EPSILON {
            // 矩阵奇异，无法求解
            return Vec::new();
        }
        // 交换行（使用 Vec::swap 避免双重可变借用）
        if pivot != col {
            mat.swap(col, pivot);
            a.swap(col, pivot);
        }
        // 消元
        let pivot_val = mat[col][col];
        for r in 0..m {
            if r == col {
                continue;
            }
            let factor = mat[r][col] / pivot_val;
            if factor == 0.0 {
                continue;
            }
            for c in col..m {
                mat[r][c] -= factor * mat[col][c];
            }
            a[r] -= factor * a[col];
        }
    }

    // 回代：由于上面已对每列消去其他行，mat 现在是对角矩阵
    let mut coeffs = vec![0.0; m];
    for i in 0..m {
        if mat[i][i].abs() < f64::EPSILON {
            return Vec::new();
        }
        coeffs[i] = a[i] / mat[i][i];
    }
    coeffs
}

/// 应用线性校准。
pub fn apply_linear(raw: f64, coeff: &CalibrationCoefficients) -> f64 {
    coeff.slope * raw + coeff.intercept
}

/// 应用多项式校准。
///
/// `coefficients = [a0, a1, a2, ...]`，计算 `a0 + a1*x + a2*x² + ...`。
pub fn apply_polynomial(raw: f64, coefficients: &[f64]) -> f64 {
    coefficients
        .iter()
        .enumerate()
        .map(|(i, &c)| c * raw.powi(i as i32))
        .sum()
}

/// 评估拟合优度（R²）。
///
/// - `points`: 校准点 `(x, y_actual)`
/// - `predict`: 预测函数 `y = predict(x)`
///
/// `R² = 1 - SS_res / SS_tot`，越接近 1 表示拟合越好。
/// 当 `SS_tot` 为 0（所有 y 相同）时，若预测也完全相同则返回 1，否则返回 0。
pub fn evaluate_fit(points: &[(f64, f64)], predict: impl Fn(f64) -> f64) -> f64 {
    if points.is_empty() {
        return 0.0;
    }
    let y_mean = points.iter().map(|(_, y)| *y).sum::<f64>() / points.len() as f64;
    let ss_tot: f64 = points.iter().map(|(_, y)| (y - y_mean).powi(2)).sum();
    let ss_res: f64 = points
        .iter()
        .map(|(x, y)| (y - predict(*x)).powi(2))
        .sum();
    if ss_tot.abs() < f64::EPSILON {
        if ss_res.abs() < f64::EPSILON {
            1.0
        } else {
            0.0
        }
    } else {
        1.0 - ss_res / ss_tot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 辅助：比较两个 f64 是否在容差范围内相等
    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn test_zero_calibration() {
        let coeff = zero_calibration(2.5);
        assert!(approx(coeff.slope, 1.0));
        assert!(approx(coeff.intercept, -2.5));
        // raw=2.5 时输出应为 0
        assert!(approx(apply_linear(2.5, &coeff), 0.0));
        // raw=12.5 时输出应为 10.0
        assert!(approx(apply_linear(12.5, &coeff), 10.0));
    }

    #[test]
    fn test_zero_span_calibration() {
        // zero_raw=1, span_raw=11, span_value=100
        // slope = 100 / (11 - 1) = 10
        // intercept = -1 * 10 = -10
        let coeff = zero_span_calibration(1.0, 11.0, 100.0);
        assert!(approx(coeff.slope, 10.0));
        assert!(approx(coeff.intercept, -10.0));
        // 验证两端
        assert!(approx(apply_linear(1.0, &coeff), 0.0));
        assert!(approx(apply_linear(11.0, &coeff), 100.0));
        // 中点
        assert!(approx(apply_linear(6.0, &coeff), 50.0));
    }

    #[test]
    fn test_linear_fit_perfect_line() {
        // 完美直线 y = 2x + 3
        let points: Vec<(f64, f64)> = vec![(0.0, 3.0), (1.0, 5.0), (2.0, 7.0), (3.0, 9.0)];
        let result = linear_fit(&points);
        assert!(approx(result.coefficients.slope, 2.0));
        assert!(approx(result.coefficients.intercept, 3.0));
        assert!(approx(result.r_squared, 1.0)); // 完美拟合 R²=1
        assert_eq!(result.residuals.len(), 4);
        for r in &result.residuals {
            assert!(r.abs() < 1e-9);
        }
    }

    #[test]
    fn test_linear_fit_with_noise() {
        // 带噪声数据，斜率约为 2，截距约为 1，R² 应小于 1 但接近 1
        let points: Vec<(f64, f64)> = vec![
            (0.0, 1.1),
            (1.0, 2.9),
            (2.0, 5.2),
            (3.0, 6.8),
            (4.0, 9.1),
        ];
        let result = linear_fit(&points);
        assert!(result.coefficients.slope > 1.8 && result.coefficients.slope < 2.2);
        assert!(
            result.coefficients.intercept > 0.8 && result.coefficients.intercept < 1.2
        );
        assert!(result.r_squared > 0.99);
        assert!(result.r_squared < 1.0);
    }

    #[test]
    fn test_linear_fit_insufficient_points() {
        let points = vec![(1.0, 2.0)];
        let result = linear_fit(&points);
        assert!(approx(result.coefficients.slope, 0.0));
        assert!(approx(result.coefficients.intercept, 0.0));
        assert!(approx(result.r_squared, 0.0));
        assert_eq!(result.points.len(), 1);
    }

    #[test]
    fn test_linear_fit_same_x_values() {
        // 所有 x 相同，分母为 0，应返回零系数
        let points = vec![(5.0, 1.0), (5.0, 2.0), (5.0, 3.0)];
        let result = linear_fit(&points);
        assert!(approx(result.coefficients.slope, 0.0));
        assert!(approx(result.coefficients.intercept, 0.0));
    }

    #[test]
    fn test_polynomial_fit_linear() {
        // 一次多项式拟合应与线性拟合一致：y = 2x + 3
        let points: Vec<(f64, f64)> = vec![(0.0, 3.0), (1.0, 5.0), (2.0, 7.0), (3.0, 9.0)];
        let coeffs = polynomial_fit(&points, 1);
        assert_eq!(coeffs.len(), 2);
        assert!(approx(coeffs[0], 3.0)); // intercept
        assert!(approx(coeffs[1], 2.0)); // slope
        // 验证预测
        assert!(approx(apply_polynomial(5.0, &coeffs), 13.0));
    }

    #[test]
    fn test_polynomial_fit_quadratic() {
        // 完美二次曲线 y = 1 + 2x + 3x²
        let points: Vec<(f64, f64)> = vec![
            (0.0, 1.0),
            (1.0, 6.0),
            (2.0, 17.0),
            (3.0, 34.0),
            (4.0, 57.0),
        ];
        let coeffs = polynomial_fit(&points, 2);
        assert_eq!(coeffs.len(), 3);
        assert!(approx(coeffs[0], 1.0)); // a0
        assert!(approx(coeffs[1], 2.0)); // a1
        assert!(approx(coeffs[2], 3.0)); // a2
        // 验证预测
        assert!(approx(apply_polynomial(5.0, &coeffs), 1.0 + 10.0 + 75.0));
    }

    #[test]
    fn test_polynomial_fit_cubic() {
        // 完美三次曲线 y = 2 - x + x² - x³
        let points: Vec<(f64, f64)> = vec![
            (-2.0, 2.0 - (-2.0) + 4.0 - (-8.0)),  // 2+2+4+8 = 16
            (-1.0, 2.0 - (-1.0) + 1.0 - (-1.0)),  // 2+1+1+1 = 5
            (0.0, 2.0),
            (1.0, 2.0 - 1.0 + 1.0 - 1.0), // 1
            (2.0, 2.0 - 2.0 + 4.0 - 8.0), // -4
        ];
        let coeffs = polynomial_fit(&points, 3);
        assert_eq!(coeffs.len(), 4);
        assert!(approx(coeffs[0], 2.0));
        assert!(approx(coeffs[1], -1.0));
        assert!(approx(coeffs[2], 1.0));
        assert!(approx(coeffs[3], -1.0));
    }

    #[test]
    fn test_polynomial_fit_insufficient_points() {
        let points = vec![(0.0, 1.0), (1.0, 2.0)];
        // degree=3 需要 4 个点，仅 2 个应返回空
        let coeffs = polynomial_fit(&points, 3);
        assert!(coeffs.is_empty());
    }

    #[test]
    fn test_polynomial_fit_degree_zero() {
        let points = vec![(0.0, 1.0), (1.0, 2.0)];
        let coeffs = polynomial_fit(&points, 0);
        assert!(coeffs.is_empty());
    }

    #[test]
    fn test_apply_polynomial() {
        let coeffs = vec![1.0, 2.0, 3.0]; // 1 + 2x + 3x²
        // x=2: 1 + 4 + 12 = 17
        assert!(approx(apply_polynomial(2.0, &coeffs), 17.0));
        // x=0: 1
        assert!(approx(apply_polynomial(0.0, &coeffs), 1.0));
    }

    #[test]
    fn test_apply_linear() {
        let coeff = CalibrationCoefficients {
            slope: 3.0,
            intercept: -2.0,
        };
        // 3*5 - 2 = 13
        assert!(approx(apply_linear(5.0, &coeff), 13.0));
    }

    #[test]
    fn test_evaluate_fit_perfect() {
        let points: Vec<(f64, f64)> = vec![(0.0, 0.0), (1.0, 2.0), (2.0, 4.0)];
        // 完美预测 y = 2x
        let r2 = evaluate_fit(&points, |x| 2.0 * x);
        assert!(approx(r2, 1.0));
    }

    #[test]
    fn test_evaluate_fit_poor() {
        let points: Vec<(f64, f64)> = vec![(0.0, 0.0), (1.0, 10.0), (2.0, 0.0)];
        // 用常数预测 y=0（即均值），R² 应为 0
        let r2 = evaluate_fit(&points, |_| 0.0);
        // y_mean = 10/3, SS_tot = (0-10/3)² + (10-10/3)² + (0-10/3)² = 200/3
        // SS_res = 0 + 100 + 0 = 100
        // R² = 1 - 100 / (200/3) = 1 - 1.5 = -0.5
        assert!(approx(r2, -0.5));
    }

    #[test]
    fn test_evaluate_fit_constant_y() {
        // 所有 y 相同，SS_tot=0
        let points: Vec<(f64, f64)> = vec![(0.0, 5.0), (1.0, 5.0), (2.0, 5.0)];
        // 预测也全部为 5，R² 应为 1
        let r2 = evaluate_fit(&points, |_| 5.0);
        assert!(approx(r2, 1.0));
        // 预测偏差，R² 应为 0
        let r2_bad = evaluate_fit(&points, |_| 4.0);
        assert!(approx(r2_bad, 0.0));
    }

    #[test]
    fn test_calibration_result_serialization() {
        let result = CalibrationResult {
            coefficients: CalibrationCoefficients {
                slope: 2.0,
                intercept: 1.0,
            },
            r_squared: 0.98,
            residuals: vec![0.1, -0.1],
            points: vec![(1.0, 3.0), (2.0, 5.0)],
        };
        let json = serde_json::to_string(&result).expect("序列化失败");
        let back: CalibrationResult = serde_json::from_str(&json).expect("反序列化失败");
        assert!(approx(back.coefficients.slope, 2.0));
        assert!(approx(back.coefficients.intercept, 1.0));
        assert!(approx(back.r_squared, 0.98));
        assert_eq!(back.residuals.len(), 2);
        assert_eq!(back.points.len(), 2);
    }

    #[test]
    fn test_linear_fit_residuals_correctness() {
        // 验证残差 = y_actual - y_predicted
        let points: Vec<(f64, f64)> = vec![(0.0, 0.0), (1.0, 2.0), (2.0, 5.0)];
        let result = linear_fit(&points);
        for (i, &(x, y)) in points.iter().enumerate() {
            let pred = result.coefficients.slope * x + result.coefficients.intercept;
            assert!(approx(result.residuals[i], y - pred));
        }
    }
}
