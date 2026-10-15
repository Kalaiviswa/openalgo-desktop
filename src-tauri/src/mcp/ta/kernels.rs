//! The indicator kernels (`_backend.py`): pure functions over `f64` slices,
//! `NaN` where a value does not exist yet.

use super::NAN;

/// Rolling reduction over complete windows; `NaN` when the window holds one
/// (`_backend.py:1919` `_roll` with `np.max` / `np.min` / `np.mean`, and the
/// window-local NaN rule of `_roll_nan`, `_backend.py:51`).
fn roll(data: &[f64], period: usize, f: impl Fn(&[f64]) -> f64) -> Vec<f64> {
    let n = data.len();
    let mut out = vec![NAN; n];
    if period == 0 || n < period {
        return out;
    }
    for i in period - 1..n {
        let w = &data[i + 1 - period..=i];
        out[i] = if w.iter().any(|x| x.is_nan()) {
            NAN
        } else {
            f(w)
        };
    }
    out
}

fn mean(w: &[f64]) -> f64 {
    w.iter().sum::<f64>() / w.len() as f64
}

/// `_backend.py:75` `sma`.
pub fn sma_k(data: &[f64], period: usize) -> Vec<f64> {
    roll(data, period, mean)
}

/// `_backend.py:89` `wma`: weights 1..=period, newest heaviest.
pub fn wma_k(data: &[f64], period: usize) -> Vec<f64> {
    let wsum = (period * (period + 1) / 2) as f64;
    roll(data, period, |w| {
        w.iter()
            .enumerate()
            .map(|(j, x)| x * (j + 1) as f64)
            .sum::<f64>()
            / wsum
    })
}

/// `_backend.py:128` `ema`: seeded at the first finite value, NaNs skipped
/// (left NaN in the output) without breaking the recursion.
pub fn ema_k(data: &[f64], period: usize) -> Vec<f64> {
    let alpha = 2.0 / (period as f64 + 1.0);
    let mut out = vec![NAN; data.len()];
    let mut prev = NAN;
    for (i, &x) in data.iter().enumerate() {
        if x.is_nan() {
            continue;
        }
        prev = if prev.is_nan() {
            x
        } else {
            alpha * x + (1.0 - alpha) * prev
        };
        out[i] = prev;
    }
    out
}

/// `_backend.py:35` `_first_clean_window`.
fn first_clean_window(data: &[f64], period: usize) -> Option<usize> {
    if period == 0 || data.len() < period {
        return None;
    }
    let mut run = 0;
    for (i, x) in data.iter().enumerate() {
        run = if x.is_nan() { 0 } else { run + 1 };
        if run == period {
            return Some(i + 1 - period);
        }
    }
    None
}

/// `_backend.py:406` `ema_wilder` (RMA): seeded with the mean of the first
/// NaN-free window, then `(prev * (p - 1) + x) / p`; a NaN repeats `prev`.
pub fn ema_wilder_k(data: &[f64], period: usize) -> Vec<f64> {
    let n = data.len();
    let mut out = vec![NAN; n];
    let Some(fv) = first_clean_window(data, period) else {
        return out;
    };
    let start = fv + period - 1;
    out[start] = data[fv..fv + period].iter().sum::<f64>() / period as f64;
    for i in start + 1..n {
        let v = data[i];
        out[i] = if v.is_nan() {
            out[i - 1]
        } else {
            (out[i - 1] * (period as f64 - 1.0) + v) / period as f64
        };
    }
    out
}

/// `_backend.py:1928` `ema_sma`: EMA seeded with the SMA of the first window.
pub fn ema_sma_k(data: &[f64], period: usize) -> Vec<f64> {
    let n = data.len();
    let mut out = vec![NAN; n];
    let Some(fv) = first_clean_window(data, period) else {
        return out;
    };
    let alpha = 2.0 / (period as f64 + 1.0);
    let start = fv + period - 1;
    let mut prev = data[fv..fv + period].iter().sum::<f64>() / period as f64;
    out[start] = prev;
    for i in start + 1..n {
        if data[i].is_nan() {
            continue;
        }
        prev = alpha * data[i] + (1.0 - alpha) * prev;
        out[i] = prev;
    }
    out
}

/// Population standard deviation per window (`_backend.py:148` `stdev`,
/// `_backend.py:1828` `_win_std`).
pub fn stdev_k(data: &[f64], period: usize) -> Vec<f64> {
    roll(data, period, |w| {
        let m = mean(w);
        (w.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / w.len() as f64)
            .max(0.0)
            .sqrt()
    })
}

/// `_backend.py:164` `true_range`.
pub fn true_range_k(high: &[f64], low: &[f64], close: &[f64]) -> Vec<f64> {
    let n = high.len();
    let mut tr = vec![NAN; n];
    if n == 0 {
        return tr;
    }
    tr[0] = high[0] - low[0];
    for i in 1..n {
        let hl = high[i] - low[i];
        let hc = (high[i] - close[i - 1]).abs();
        let lc = (low[i] - close[i - 1]).abs();
        tr[i] = if hl.is_nan() || hc.is_nan() || lc.is_nan() {
            NAN
        } else {
            hl.max(hc).max(lc)
        };
    }
    tr
}

/// `_backend.py:186` `atr_wilder`.
pub fn atr_k(high: &[f64], low: &[f64], close: &[f64], period: usize) -> Vec<f64> {
    if period == 0 || high.len() < period {
        return vec![NAN; high.len()];
    }
    ema_wilder_k(&true_range_k(high, low, close), period)
}

/// `_backend.py:201` `rsi` (Wilder averages seeded on the first clean
/// `period + 1` values).
pub fn rsi_k(data: &[f64], period: usize) -> Vec<f64> {
    let n = data.len();
    let mut out = vec![NAN; n];
    if period == 0 || n < period + 1 {
        return out;
    }
    let Some(fv) = first_clean_window(data, period + 1) else {
        return out;
    };
    let deltas: Vec<f64> = data.windows(2).map(|w| w[1] - w[0]).collect();
    let gain = |d: f64| if d > 0.0 { d } else { 0.0 };
    let loss = |d: f64| if d < 0.0 { -d } else { 0.0 };
    let p = period as f64;
    let mut ag = deltas[fv..fv + period]
        .iter()
        .map(|&d| gain(d))
        .sum::<f64>()
        / p;
    let mut al = deltas[fv..fv + period]
        .iter()
        .map(|&d| loss(d))
        .sum::<f64>()
        / p;
    let value = |ag: f64, al: f64| {
        if al == 0.0 {
            100.0
        } else {
            100.0 - 100.0 / (1.0 + ag / al)
        }
    };
    let start = fv + period;
    out[start] = value(ag, al);
    for i in start..n - 1 {
        let d = deltas[i];
        if d.is_nan() {
            continue;
        }
        ag = (ag * (p - 1.0) + gain(d)) / p;
        al = (al * (p - 1.0) + loss(d)) / p;
        out[i + 1] = value(ag, al);
    }
    out
}

/// `_backend.py:267` `cci`.
pub fn cci_k(high: &[f64], low: &[f64], close: &[f64], period: usize) -> Vec<f64> {
    let n = close.len();
    let mut out = vec![NAN; n];
    if period == 0 || n < period {
        return out;
    }
    let tp: Vec<f64> = (0..n)
        .map(|i| (high[i] + low[i] + close[i]) / 3.0)
        .collect();
    let p = period as f64;
    let mut rsum: f64 = tp[..period].iter().sum();
    for i in period - 1..n {
        if i > period - 1 {
            rsum = rsum + tp[i] - tp[i - period];
        }
        let m = rsum / p;
        let md = tp[i + 1 - period..=i]
            .iter()
            .map(|x| (x - m).abs())
            .sum::<f64>()
            / p;
        out[i] = if md != 0.0 {
            (tp[i] - m) / (0.015 * md)
        } else {
            0.0
        };
    }
    out
}

fn max_of(w: &[f64]) -> f64 {
    w.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
}
fn min_of(w: &[f64]) -> f64 {
    w.iter().cloned().fold(f64::INFINITY, f64::min)
}

/// Rolling highest (`utils.py:83`, `_backend.py:2022`).
pub fn highest_k(data: &[f64], period: usize) -> Vec<f64> {
    roll(data, period, max_of)
}

/// Rolling lowest (`utils.py:129`, `_backend.py:2029`).
pub fn lowest_k(data: &[f64], period: usize) -> Vec<f64> {
    roll(data, period, min_of)
}

/// `_backend.py:299` `williams_r`.
pub fn williams_r_k(high: &[f64], low: &[f64], close: &[f64], period: usize) -> Vec<f64> {
    let hh = highest_k(high, period);
    let ll = lowest_k(low, period);
    let mut out = vec![NAN; close.len()];
    for i in period.saturating_sub(1)..close.len() {
        out[i] = if hh[i] != ll[i] {
            -100.0 * (hh[i] - close[i]) / (hh[i] - ll[i])
        } else {
            -50.0
        };
    }
    out
}

/// `_backend.py:249` `stochastic`: slow %K = SMA(fast %K, smooth_k),
/// %D = SMA(slow %K, d_period).
pub fn stochastic_k(
    high: &[f64],
    low: &[f64],
    close: &[f64],
    k_period: usize,
    smooth_k: usize,
    d_period: usize,
) -> (Vec<f64>, Vec<f64>) {
    let n = close.len();
    let hh = highest_k(high, k_period);
    let ll = lowest_k(low, k_period);
    let fk = k_period - 1;
    let mut fast = vec![NAN; n];
    for i in fk..n {
        fast[i] = if hh[i] != ll[i] {
            100.0 * (close[i] - ll[i]) / (hh[i] - ll[i])
        } else {
            50.0
        };
    }
    let mut slow_k = vec![NAN; n];
    if fk < n {
        for (j, v) in sma_k(&fast[fk..], smooth_k).into_iter().enumerate() {
            slow_k[fk + j] = v;
        }
    }
    let off = fk + smooth_k - 1;
    let mut slow_d = vec![NAN; n];
    if off < n {
        for (j, v) in sma_k(&slow_k[off..], d_period).into_iter().enumerate() {
            slow_d[off + j] = v;
        }
    }
    (slow_k, slow_d)
}

/// `_backend.py:2036` `supertrend`: (line, direction) with direction -1 up
/// and +1 down, as TradingView.
pub fn supertrend_k(
    high: &[f64],
    low: &[f64],
    close: &[f64],
    period: usize,
    mult: f64,
) -> (Vec<f64>, Vec<f64>) {
    let n = close.len();
    let mut st = vec![NAN; n];
    let mut dr = vec![NAN; n];
    let atr = atr_k(high, low, close, period);
    let fv = period - 1;
    if fv >= n {
        return (st, dr);
    }
    let ub: Vec<f64> = (0..n)
        .map(|i| (high[i] + low[i]) / 2.0 + mult * atr[i])
        .collect();
    let lb: Vec<f64> = (0..n)
        .map(|i| (high[i] + low[i]) / 2.0 - mult * atr[i])
        .collect();
    let mut fu = vec![NAN; n];
    let mut fl = vec![NAN; n];
    fu[fv] = ub[fv];
    fl[fv] = lb[fv];
    dr[fv] = 1.0;
    st[fv] = ub[fv];
    for i in fv + 1..n {
        fl[i] = if lb[i] > fl[i - 1] || close[i - 1] < fl[i - 1] {
            lb[i]
        } else {
            fl[i - 1]
        };
        fu[i] = if ub[i] < fu[i - 1] || close[i - 1] > fu[i - 1] {
            ub[i]
        } else {
            fu[i - 1]
        };
        dr[i] = if st[i - 1] == fu[i - 1] {
            if close[i] > fu[i] {
                -1.0
            } else {
                1.0
            }
        } else if close[i] < fl[i] {
            1.0
        } else {
            -1.0
        };
        st[i] = if dr[i] == -1.0 { fl[i] } else { fu[i] };
    }
    (st, dr)
}

/// `_backend.py:2101` `ichimoku`: conversion, base, leading A and B
/// (shifted forward `disp - 1`), lagging (shifted back `disp - 1`).
pub fn ichimoku_k(
    high: &[f64],
    low: &[f64],
    close: &[f64],
    conv: usize,
    base: usize,
    span2: usize,
    disp: i64,
) -> Vec<Vec<f64>> {
    let n = close.len();
    let don = |p: usize| -> Vec<f64> {
        let h = highest_k(high, p);
        let l = lowest_k(low, p);
        (0..n).map(|i| (h[i] + l[i]) / 2.0).collect()
    };
    let conversion = don(conv);
    let base_line = don(base);
    let lead1: Vec<f64> = (0..n)
        .map(|i| (conversion[i] + base_line[i]) / 2.0)
        .collect();
    let lead2 = don(span2);
    let off = disp - 1;
    let shift_fwd = |src: &[f64]| -> Vec<f64> {
        let mut out = vec![NAN; n];
        if off == 0 {
            out.copy_from_slice(src);
        } else if off > 0 && (off as usize) < n {
            let o = off as usize;
            out[o..].copy_from_slice(&src[..n - o]);
        }
        out
    };
    let la = shift_fwd(&lead1);
    let lb = shift_fwd(&lead2);
    let mut lag = vec![NAN; n];
    let offlag = -disp + 1;
    if offlag < 0 {
        let sh = offlag.unsigned_abs() as usize;
        if sh < n {
            lag[..n - sh].copy_from_slice(&close[sh..]);
        }
    } else if offlag > 0 {
        let o = offlag as usize;
        if o < n {
            lag[o..].copy_from_slice(&close[..n - o]);
        }
    } else {
        lag.copy_from_slice(close);
    }
    vec![conversion, base_line, la, lb, lag]
}

/// `_backend.py:728` `adx`: (+DI, -DI, ADX).
pub fn adx_k(
    high: &[f64],
    low: &[f64],
    close: &[f64],
    period: usize,
) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let n = close.len();
    let tr = true_range_k(high, low, close);
    let mut dmp = vec![0.0; n];
    let mut dmm = vec![0.0; n];
    for i in 1..n {
        let up = high[i] - high[i - 1];
        let dn = low[i - 1] - low[i];
        dmp[i] = if up > dn && up > 0.0 { up } else { 0.0 };
        dmm[i] = if dn > up && dn > 0.0 { dn } else { 0.0 };
    }
    let sa = ema_wilder_k(&tr, period);
    let sp = ema_wilder_k(&dmp, period);
    let sm = ema_wilder_k(&dmm, period);
    let mut dip = vec![NAN; n];
    let mut dim = vec![NAN; n];
    let mut dx = vec![NAN; n];
    for i in period.saturating_sub(1)..n {
        if !sa[i].is_nan() && sa[i] > 0.0 {
            dip[i] = sp[i] / sa[i] * 100.0;
            dim[i] = sm[i] / sa[i] * 100.0;
            let s = dip[i] + dim[i];
            if s > 0.0 {
                dx[i] = (dip[i] - dim[i]).abs() / s * 100.0;
            }
        }
    }
    let adx = ema_wilder_k(&dx, period);
    (dip, dim, adx)
}

/// `_backend.py:781` `pivot_points`: pivot, r1, s1, r2, s2, r3, s3 per bar.
pub fn pivot_points_k(high: &[f64], low: &[f64], close: &[f64]) -> Vec<Vec<f64>> {
    let n = close.len();
    let mut out = vec![vec![NAN; n]; 7];
    for i in 0..n {
        let (h, l) = (high[i], low[i]);
        let p = (h + l + close[i]) / 3.0;
        out[0][i] = p;
        out[1][i] = 2.0 * p - l;
        out[2][i] = 2.0 * p - h;
        out[3][i] = p + (h - l);
        out[4][i] = p - (h - l);
        out[5][i] = h + 2.0 * (p - l);
        out[6][i] = l - 2.0 * (h - p);
    }
    out
}

/// `_backend.py:891` `_linreg_end`: the fitted value at the window's end.
fn linreg_end(y: &[f64]) -> f64 {
    let p = y.len() as f64;
    let sx: f64 = (0..y.len()).map(|x| x as f64).sum();
    let sy: f64 = y.iter().sum();
    let sxy: f64 = y.iter().enumerate().map(|(x, v)| x as f64 * v).sum();
    let sx2: f64 = (0..y.len()).map(|x| (x * x) as f64).sum();
    let den = p * sx2 - sx * sx;
    if den != 0.0 {
        let slope = (p * sxy - sx * sy) / den;
        let intercept = (sy - slope * sx) / p;
        slope * (p - 1.0) + intercept
    } else {
        y[y.len() - 1]
    }
}

/// `_backend.py:919` `lrslope`: change of the regression end value per bar.
pub fn lrslope_k(data: &[f64], period: usize, interval: f64) -> Vec<f64> {
    let n = data.len();
    let mut out = vec![NAN; n];
    for i in period..n {
        out[i] =
            (linreg_end(&data[i + 1 - period..=i]) - linreg_end(&data[i - period..i])) / interval;
    }
    out
}

/// `_backend.py:950` `correl`: rolling Pearson correlation.
pub fn correl_k(a: &[f64], b: &[f64], period: usize) -> Vec<f64> {
    let n = a.len();
    let mut out = vec![NAN; n];
    if period == 0 {
        return out;
    }
    for i in period - 1..n {
        let x = &a[i + 1 - period..=i];
        let y = &b[i + 1 - period..=i];
        let (mx, my) = (mean(x), mean(y));
        let num: f64 = x.iter().zip(y).map(|(p, q)| (p - mx) * (q - my)).sum();
        let sxx: f64 = x.iter().map(|p| (p - mx) * (p - mx)).sum();
        let syy: f64 = y.iter().map(|q| (q - my) * (q - my)).sum();
        let den = (sxx * syy).sqrt();
        out[i] = if den > 0.0 { num / den } else { 0.0 };
    }
    out
}

/// `_backend.py:967` `beta`: rolling covariance of price changes over the
/// market's variance.
pub fn beta_k(asset: &[f64], market: &[f64], period: usize) -> Vec<f64> {
    let n = asset.len();
    let mut out = vec![NAN; n];
    let mut ar = vec![NAN; n];
    let mut mr = vec![NAN; n];
    for i in 1..n {
        ar[i] = asset[i] - asset[i - 1];
        mr[i] = market[i] - market[i - 1];
    }
    for i in period..n {
        let aw = &ar[i + 1 - period..=i];
        let mw = &mr[i + 1 - period..=i];
        let (ma, mm) = (mean(aw), mean(mw));
        let mut cov = 0.0;
        let mut var = 0.0;
        for j in 0..period {
            cov += (aw[j] - ma) * (mw[j] - mm);
            var += (mw[j] - mm) * (mw[j] - mm);
        }
        cov /= period as f64;
        var /= period as f64;
        out[i] = if var > 0.0 { cov / var } else { 0.0 };
    }
    out
}

/// `_backend.py:1884` `hv`: annualised stdev of log returns, in percent.
pub fn hv_k(close: &[f64], length: usize, annual: f64, per: f64) -> Vec<f64> {
    let n = close.len();
    let mut lr = vec![NAN; n];
    for i in 1..n {
        if close[i - 1] > 0.0 && close[i] > 0.0 {
            lr[i] = (close[i] / close[i - 1]).ln();
        }
    }
    stdev_k(&lr, length)
        .into_iter()
        .map(|s| 100.0 * s * (annual / per).sqrt())
        .collect()
}

/// `utils.py:25` `crossover`: `a` crosses above `b` on this bar.
pub fn crossover(a: &[f64], b: &[f64]) -> Vec<bool> {
    let mut out = vec![false; a.len()];
    for i in 1..a.len() {
        let ok = ![a[i], b[i], a[i - 1], b[i - 1]].iter().any(|x| x.is_nan());
        out[i] = ok && a[i] > b[i] && a[i - 1] <= b[i - 1];
    }
    out
}

/// `utils.py:54` `crossunder`.
pub fn crossunder(a: &[f64], b: &[f64]) -> Vec<bool> {
    let mut out = vec![false; a.len()];
    for i in 1..a.len() {
        let ok = ![a[i], b[i], a[i - 1], b[i - 1]].iter().any(|x| x.is_nan());
        out[i] = ok && a[i] < b[i] && a[i - 1] >= b[i - 1];
    }
    out
}
