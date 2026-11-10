"""Train the learned grade: gradient-boosted trees, features -> fitted correction.
Writes grade_model.json (format read by grade-core/src/learn.rs) and the held-out id list."""
import json, sys, numpy as np, random
from sklearn.ensemble import GradientBoostingRegressor

rows = [json.loads(l) for l in open(sys.argv[1])]
ids = sorted({r['id'] for r in rows})
random.Random(7).shuffle(ids)
n_val = int(len(ids) * float(sys.argv[3]) ) if len(sys.argv) > 3 else 0
val = set(ids[:n_val])
tr = [r for r in rows if r['id'] not in val]
X = np.array([r['features'] for r in tr], np.float32)
Y = np.array([r['fit'] for r in tr], np.float32)
# trust good fits more (a bad fit is a noisy target)
w = None
print('train', len(tr), 'features', X.shape[1])
NAMES = ["ev", "temp", "tint", "black", "white", "shadows", "contrast", "knee", "vibrance", "sat"]
targets = []
for k in range(Y.shape[1]):
    m = GradientBoostingRegressor(n_estimators=int(sys.argv[4]) if len(sys.argv) > 4 else 250, max_depth=3, learning_rate=0.05, subsample=0.8, min_samples_leaf=8, loss='huber', random_state=k)
    m.fit(X, Y[:, k], sample_weight=w)
    init = float(m.init_.constant_.ravel()[0]) if hasattr(m.init_, 'constant_') else float(np.median(Y[:, k]))
    trees = []
    for est in m.estimators_[:, 0]:
        t = est.tree_
        trees.append({'f': [int(f) if f >= 0 else -1 for f in t.feature], 't': [float(v) for v in t.threshold],
                      'l': [int(v) for v in t.children_left], 'r': [int(v) for v in t.children_right], 'v': [float(v[0][0]) for v in t.value]})
    # sanity: our own evaluation equals sklearn's
    def pred(x):
        y = init
        for tr_ in trees:
            i = 0
            while tr_['f'][i] >= 0:
                i = tr_['l'][i] if x[tr_['f'][i]] <= tr_['t'][i] else tr_['r'][i]
            y += m.learning_rate * tr_['v'][i]
        return y
    assert abs(pred(X[0]) - m.predict(X[:1])[0]) < 1e-3, (pred(X[0]), m.predict(X[:1])[0])
    targets.append({'init': init, 'lr': m.learning_rate, 'trees': trees})
    print(NAMES[k], 'train R2 %.2f' % m.score(X, Y[:, k]))
mean = X.mean(0); std = X.std(0) + 1e-6
knn = {'k': 30, 'mean': [float(v) for v in mean], 'std': [float(v) for v in std],
       'x': [[round(float(v), 5) for v in row] for row in (X - mean) / std], 'y': [[round(float(v), 5) for v in row] for row in Y]}
json.dump({'n_features': X.shape[1], 'targets': targets, 'knn': knn}, open(sys.argv[2], 'w'), separators=(',', ':'))
open('val_ids.txt', 'w').write('\n'.join(sorted(val)))
print('wrote', sys.argv[2], 'val ids', len(val))
