import matplotlib.pyplot as plt
import numpy as np
import seaborn as sns
from matplotlib.gridspec import GridSpec
from matplotlib.patches import Patch

sns.set_theme(style="darkgrid")
plt.rcParams.update({
    'figure.facecolor': '#0d1117',
    'axes.facecolor': '#161b22',
    'text.color': '#e6edf3',
    'axes.labelcolor': '#e6edf3',
    'xtick.color': '#8b949e',
    'ytick.color': '#8b949e',
    'grid.color': '#21262d',
    'axes.edgecolor': '#30363d',
    'font.family': 'sans-serif',
    'font.size': 11,
})

# === ALL SUBREDDITS (consistent across every chart) ===
data = {
    'r/ClaudeAI':      {'subs': 509, 'avg_up': 8,   'avg_com': 5.2,  'max_up': 113,  'relevance': 10, 'tier': 1},
    'r/ChatGPTCoding':  {'subs': 357, 'avg_up': 46,  'avg_com': 25.0, 'max_up': 883,  'relevance': 9,  'tier': 1},
    'r/LocalLLaMA':    {'subs': 628, 'avg_up': 3,   'avg_com': 4.2,  'max_up': 29,   'relevance': 9,  'tier': 1},
    'r/cursor':        {'subs': 122, 'avg_up': 12,  'avg_com': 13.1, 'max_up': 129,  'relevance': 8,  'tier': 2},
    'r/rust':          {'subs': 388, 'avg_up': 19,  'avg_com': 8.7,  'max_up': 133,  'relevance': 8,  'tier': 2},
    'r/commandline':   {'subs': 115, 'avg_up': 13,  'avg_com': 4.4,  'max_up': 230,  'relevance': 8,  'tier': 2},
    'r/opensource':    {'subs': 326, 'avg_up': 18,  'avg_com': 6.1,  'max_up': 185,  'relevance': 7,  'tier': 3},
    'r/neovim':        {'subs': 148, 'avg_up': 28,  'avg_com': 11.9, 'max_up': 278,  'relevance': 7,  'tier': 3},
    'r/selfhosted':    {'subs': 698, 'avg_up': 18,  'avg_com': 9.0,  'max_up': 248,  'relevance': 6,  'tier': 3},
    'r/ollama':        {'subs': 101, 'avg_up': 9,   'avg_com': 5.3,  'max_up': 84,   'relevance': 6,  'tier': 3},
    'r/programming':   {'subs': 6841,'avg_up': 125, 'avg_com': 26.4, 'max_up': 2566, 'relevance': 5,  'tier': 4},
    'r/linux':         {'subs': 1821,'avg_up': 161, 'avg_com': 32.0, 'max_up': 890,  'relevance': 5,  'tier': 4},
    'r/artificial':    {'subs': 1224,'avg_up': 60,  'avg_com': 24.0, 'max_up': 569,  'relevance': 5,  'tier': 4},
    'r/MachineLearning':{'subs':3024,'avg_up': 23,  'avg_com': 11.7, 'max_up': 208,  'relevance': 5,  'tier': 4},
    'r/SideProject':   {'subs': 629, 'avg_up': 1,   'avg_com': 0.6,  'max_up': 4,    'relevance': 4,  'tier': 5},
}

tier_palette = {1: '#58a6ff', 2: '#3fb950', 3: '#d2a8ff', 4: '#f0883e', 5: '#f85149'}
tier_names = {1: 'Tier 1: Perfect Fit', 2: 'Tier 2: Strong Fit', 3: 'Tier 3: Good Fit',
              4: 'Tier 4: Broad Reach', 5: 'Tier 5: Skip'}

subs_list = list(data.keys())
colors = [tier_palette[data[s]['tier']] for s in subs_list]

# Hour data (Pacific) — ALL subs
hour_data = {
    'r/ClaudeAI':       [4,5,8,8,5,11,13,11,12,10,11,2,0,0,0,0,0,0,0,0,0,0,0,0],
    'r/ChatGPTCoding':  [1,4,9,4,4,3,3,5,4,5,8,7,7,5,4,2,7,4,4,2,2,1,2,3],
    'r/LocalLLaMA':     [0,3,2,5,6,6,5,12,11,6,7,8,0,0,0,0,3,6,7,0,6,4,3,0],
    'r/cursor':         [2,1,5,1,5,5,8,8,8,7,9,4,9,2,5,2,2,4,3,1,4,2,2,1],
    'r/rust':           [4,5,8,6,5,7,6,9,8,7,8,4,5,2,2,2,3,2,2,1,1,2,1,0],
    'r/commandline':    [1,3,6,3,7,9,8,3,4,6,3,4,4,8,6,5,1,3,2,4,4,2,1,3],
    'r/opensource':      [1,3,7,3,6,1,4,4,9,5,6,3,7,10,5,7,6,3,1,0,3,2,2,2],
    'r/neovim':         [4,5,5,3,3,6,8,1,4,3,8,8,3,4,7,2,4,7,2,1,3,4,3,2],
    'r/selfhosted':     [3,3,5,4,7,5,4,6,8,11,8,7,3,3,2,5,2,2,2,2,3,3,1,1],
    'r/ollama':         [2,6,6,6,3,2,4,7,10,6,5,5,8,5,2,3,6,3,2,2,2,1,2,2],
    'r/programming':    [3,3,4,7,7,11,8,6,6,6,2,10,4,1,1,2,5,1,2,0,2,4,3,2],
    'r/linux':          [3,4,6,2,4,3,3,1,4,7,7,10,6,7,5,6,5,5,5,1,1,2,2,1],
    'r/artificial':     [2,4,8,5,5,0,5,7,7,5,4,6,3,8,3,3,2,3,6,2,4,3,3,2],
    'r/MachineLearning':[7,3,2,3,4,4,7,1,11,6,3,7,4,5,6,3,3,6,3,1,2,2,4,3],
    'r/SideProject':    [0,0,0,0,0,0,0,0,13,11,18,13,11,8,10,11,5,0,0,0,0,0,0,0],
}

# Day data
day_names = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun']
day_data = {
    'r/ChatGPTCoding': [19,17,15,12,12,9,16],
    'r/cursor':        [26,21,12,0,0,21,20],
    'r/commandline':   [19,13,15,12,13,11,17],
    'r/opensource':     [14,20,11,11,17,13,14],
    'r/neovim':        [19,17,16,9,7,9,23],
    'r/rust':          [23,39,20,0,0,0,18],
    'r/ollama':        [11,20,18,15,16,8,12],
    'r/linux':         [8,24,25,11,13,11,8],
    'r/artificial':    [18,22,16,10,13,11,10],
    'r/MachineLearning':[21,23,11,7,14,12,12],
}

# ======================== FIGURE ========================
fig = plt.figure(figsize=(24, 36))
fig.suptitle('jcode Reddit Strategy Dashboard', fontsize=28, fontweight='bold',
             color='#58a6ff', y=0.985)
fig.text(0.5, 0.979, 'Complete analysis of 15 subreddits for promoting jcode (Rust AI coding agent CLI)  |  All times Pacific',
         ha='center', fontsize=13, color='#8b949e')

gs = GridSpec(6, 2, figure=fig, hspace=0.32, wspace=0.28,
              top=0.965, bottom=0.02, left=0.09, right=0.95)

# ---- LEGEND (shared) ----
legend_elements = [Patch(facecolor=tier_palette[t], label=tier_names[t]) for t in [1,2,3,4,5]]

# ========== 1. COMPOSITE RANKING ==========
ax1 = fig.add_subplot(gs[0, :])
composite = []
for s in subs_list:
    d = data[s]
    composite.append(d['relevance'] * (d['avg_up'] + d['avg_com'] * 2))
sort_idx = np.argsort(composite)[::-1]
sorted_subs = [subs_list[i] for i in sort_idx]
sorted_composite = [composite[i] for i in sort_idx]
sorted_colors = [colors[i] for i in sort_idx]

bars = ax1.barh(range(len(sorted_subs)), sorted_composite, color=sorted_colors, edgecolor='none', height=0.7)
ax1.set_yticks(range(len(sorted_subs)))
ax1.set_yticklabels(sorted_subs, fontsize=11, fontweight='bold')
ax1.invert_yaxis()
ax1.set_xlabel('Composite Score  =  Relevance x (Avg Upvotes + 2 x Avg Comments)', fontsize=11)
ax1.set_title('OVERALL RANKING', fontsize=18, fontweight='bold', pad=15)
for i, (bar, val) in enumerate(zip(bars, sorted_composite)):
    ax1.text(val + max(sorted_composite)*0.01, i, f'{val:.0f}', va='center', fontsize=10, color='#e6edf3')
ax1.legend(handles=legend_elements, loc='lower right', fontsize=9, facecolor='#161b22', edgecolor='#30363d')

# ========== 2. SUBSCRIBERS vs ENGAGEMENT ==========
ax2 = fig.add_subplot(gs[1, 0])
subs_k = [data[s]['subs'] for s in subs_list]
avg_up = [data[s]['avg_up'] for s in subs_list]
relevances = [data[s]['relevance'] for s in subs_list]
sizes = [r*35 for r in relevances]

ax2.scatter(subs_k, avg_up, s=sizes, c=colors, alpha=0.85, edgecolors='white', linewidth=0.5, zorder=5)
for i, s in enumerate(subs_list):
    ax2.annotate(s.replace('r/', ''), (subs_k[i], avg_up[i]), fontsize=7.5, color='#c9d1d9',
                 xytext=(6, 4), textcoords='offset points')
ax2.set_xlabel('Subscribers (K)', fontsize=11)
ax2.set_ylabel('Avg Upvotes per Post', fontsize=11)
ax2.set_title('SUBSCRIBERS vs ENGAGEMENT', fontsize=14, fontweight='bold')
ax2.set_xscale('log')

# ========== 3. AVG COMMENTS ==========
ax3 = fig.add_subplot(gs[1, 1])
avg_com = [data[s]['avg_com'] for s in subs_list]
sort_c = np.argsort(avg_com)[::-1]
ax3.barh(range(len(subs_list)), [avg_com[i] for i in sort_c],
         color=[colors[i] for i in sort_c], edgecolor='none', height=0.65)
ax3.set_yticks(range(len(subs_list)))
ax3.set_yticklabels([subs_list[i] for i in sort_c], fontsize=10)
ax3.invert_yaxis()
ax3.set_xlabel('Avg Comments per Post', fontsize=11)
ax3.set_title('DISCUSSION DEPTH', fontsize=14, fontweight='bold')
for i, idx in enumerate(sort_c):
    ax3.text(avg_com[idx] + 0.3, i, f'{avg_com[idx]:.1f}', va='center', fontsize=9, color='#8b949e')

# ========== 4. HEATMAP — ALL SUBS ==========
ax4 = fig.add_subplot(gs[2, :])
heat_subs = list(hour_data.keys())
heat_matrix = np.array([hour_data[s] for s in heat_subs], dtype=float)
# Normalize each row
row_sums = heat_matrix.sum(axis=1, keepdims=True)
row_sums[row_sums == 0] = 1
heat_norm = heat_matrix / row_sums * 100

im = ax4.imshow(heat_norm, cmap='YlOrRd', aspect='auto', interpolation='nearest')
ax4.set_yticks(range(len(heat_subs)))
ax4.set_yticklabels(heat_subs, fontsize=10)
ax4.set_xticks(range(24))
ax4.set_xticklabels([f'{h}' for h in range(24)], fontsize=9)
ax4.set_xlabel('Hour of Day (Pacific Time)', fontsize=12)
ax4.set_title('POSTING ACTIVITY HEATMAP BY HOUR', fontsize=16, fontweight='bold', pad=12)
cbar = plt.colorbar(im, ax=ax4, shrink=0.5, pad=0.02)
cbar.set_label('% of posts in that hour', color='#8b949e', fontsize=10)
cbar.ax.yaxis.set_tick_params(color='#8b949e')
plt.setp(plt.getp(cbar.ax.axes, 'yticklabels'), color='#8b949e')

# Mark peak hour per sub
for i in range(len(heat_subs)):
    row = heat_norm[i]
    if row.max() > 0:
        peak_h = np.argmax(row)
        ax4.text(peak_h, i, '*', ha='center', va='center', fontsize=16, color='black', fontweight='bold')

# Add morning/afternoon/evening labels
ax4.axvline(x=5.5, color='#58a6ff', linewidth=0.5, alpha=0.4, linestyle='--')
ax4.axvline(x=11.5, color='#f0883e', linewidth=0.5, alpha=0.4, linestyle='--')
ax4.axvline(x=17.5, color='#d2a8ff', linewidth=0.5, alpha=0.4, linestyle='--')
ax4.text(2.5, -0.8, 'Late Night', ha='center', fontsize=8, color='#8b949e')
ax4.text(8.5, -0.8, 'Morning', ha='center', fontsize=8, color='#58a6ff')
ax4.text(14.5, -0.8, 'Afternoon', ha='center', fontsize=8, color='#f0883e')
ax4.text(20.5, -0.8, 'Evening', ha='center', fontsize=8, color='#d2a8ff')

# ========== 5. DAY OF WEEK ==========
ax5 = fig.add_subplot(gs[3, 0])
day_subs_list = list(day_data.keys())
x = np.arange(7)
n = len(day_subs_list)
width = 0.8 / n
cmap_day = plt.cm.Set2
for i, sub in enumerate(day_subs_list):
    vals = day_data[sub]
    total = sum(vals)
    if total == 0: continue
    pcts = [v/total*100 for v in vals]
    c = tier_palette[data[sub]['tier']]
    ax5.bar(x + i*width - n*width/2, pcts, width, label=sub.replace('r/', ''), color=c, alpha=0.7, edgecolor='none')
ax5.set_xticks(x)
ax5.set_xticklabels(day_names, fontsize=11, fontweight='bold')
ax5.set_ylabel('% of posts', fontsize=11)
ax5.set_title('DAY OF WEEK DISTRIBUTION', fontsize=14, fontweight='bold')
ax5.legend(fontsize=6.5, facecolor='#161b22', edgecolor='#30363d', loc='upper right', ncol=2)

# ========== 6. VIRAL POTENTIAL ==========
ax6 = fig.add_subplot(gs[3, 1])
max_up = [data[s]['max_up'] for s in subs_list]
sort_m = np.argsort(max_up)[::-1]
ax6.barh(range(len(subs_list)), [max_up[i] for i in sort_m],
         color=[colors[i] for i in sort_m], edgecolor='none', height=0.65)
ax6.set_yticks(range(len(subs_list)))
ax6.set_yticklabels([subs_list[i] for i in sort_m], fontsize=10)
ax6.invert_yaxis()
ax6.set_xlabel('Max Upvotes (Recent Posts)', fontsize=11)
ax6.set_title('VIRAL POTENTIAL', fontsize=14, fontweight='bold')
for i, idx in enumerate(sort_m):
    ax6.text(max_up[idx] + 20, i, f'{max_up[idx]:,}', va='center', fontsize=9, color='#8b949e')

# ========== 7. BEST TIME TO POST (visual timeline) ==========
ax_time = fig.add_subplot(gs[4, :])
best_times = {
    'r/ClaudeAI':       (6, 9,   'Tue-Wed'),
    'r/ChatGPTCoding':  (10, 12, 'Monday'),
    'r/LocalLLaMA':     (7, 9,   'Tue-Wed'),
    'r/cursor':         (10, 12, 'Monday'),
    'r/rust':           (7, 10,  'Tuesday'),
    'r/commandline':    (5, 6,   'Mon/Sun'),
    'r/opensource':      (8, 13,  'Tue/Fri'),
    'r/neovim':         (6, 11,  'Sun/Mon'),
    'r/selfhosted':     (8, 10,  'Tuesday'),
    'r/ollama':         (8, 12,  'Tuesday'),
    'r/programming':    (5, 8,   'Monday'),
    'r/linux':          (9, 12,  'Tue-Wed'),
    'r/artificial':     (7, 9,   'Tuesday'),
    'r/MachineLearning':(8, 11,  'Tuesday'),
    'r/SideProject':    (9, 12,  'Wed'),
}

y_positions = list(range(len(best_times)))
for i, (sub, (start, end, day)) in enumerate(best_times.items()):
    c = tier_palette[data[sub]['tier']]
    ax_time.barh(i, end - start, left=start, height=0.6, color=c, alpha=0.85, edgecolor='white', linewidth=0.5)
    ax_time.text(end + 0.3, i, f'{start}am-{end}{"pm" if end >= 12 else "am"} ({day})',
                 va='center', fontsize=9, color='#c9d1d9')

ax_time.set_yticks(y_positions)
ax_time.set_yticklabels(list(best_times.keys()), fontsize=10)
ax_time.invert_yaxis()
ax_time.set_xlim(0, 24)
ax_time.set_xticks(range(0, 25, 2))
ax_time.set_xticklabels([f'{h}:00' for h in range(0, 25, 2)], fontsize=9)
ax_time.set_xlabel('Pacific Time', fontsize=12)
ax_time.set_title('BEST TIME TO POST (Pacific)', fontsize=16, fontweight='bold', pad=12)
ax_time.axvline(x=8, color='#3fb950', linewidth=1, alpha=0.3, linestyle='--')
ax_time.axvline(x=12, color='#f0883e', linewidth=1, alpha=0.3, linestyle='--')
ax_time.legend(handles=legend_elements, loc='upper right', fontsize=8, facecolor='#161b22', edgecolor='#30363d')

# ========== 8. STRATEGY TABLE ==========
ax7 = fig.add_subplot(gs[5, :])
ax7.axis('off')

schedule = [
    ['Subreddit', 'Subs', 'Best Day', 'Best Time', 'Approach', 'Score'],
    ['r/ClaudeAI',      '509K', 'Tue-Wed', '6-9am',    '"Built with Claude" flair',      '675'],
    ['r/ChatGPTCoding', '357K', 'Monday',  '10am-12pm','Demo video, compare to Cursor',  '675'],
    ['r/LocalLLaMA',    '628K', 'Tue-Wed', '7-9am',    'Technical deep-dive, OSS angle', '103'],
    ['r/cursor',        '122K', 'Monday',  '10am-12pm','CLI alternative to Cursor',      '305'],
    ['r/rust',          '388K', 'Tuesday', '7-10am',   'Project flair, Rust internals',  '297'],
    ['r/commandline',   '115K', 'Mon/Sun', '5am / 1pm','GIF demo, CLI showcase',         '174'],
    ['r/neovim',        '148K', 'Sunday',  '6am/10am', 'Terminal-first, vim integration', '363'],
    ['r/opensource',     '326K', 'Tue/Fri', '8am / 1pm','OSS launch announcement',        '212'],
    ['r/selfhosted',    '698K', 'Tuesday', '8-10am',   'Self-hostable AI coding agent',  '216'],
    ['r/ollama',        '101K', 'Tuesday', '8am-12pm', 'Local model integration angle',  '95'],
    ['r/programming',  '6.8M', 'Monday',  '5-8am',    'Blog post / deep technical',     '888'],
    ['r/linux',        '1.8M', 'Tue-Wed', '9am-12pm', 'Linux-native CLI tool angle',    '1125'],
    ['r/artificial',   '1.2M', 'Tuesday', '7-9am',    'AI agent capabilities showcase',  '540'],
    ['r/MachineLearning','3M', 'Tuesday', '8-11am',   'Technical architecture post',     '233'],
    ['r/SideProject',   '629K', 'Any',    '9am-3pm',  'SKIP - zero engagement',          '9'],
]

table = ax7.table(cellText=schedule[1:], colLabels=schedule[0],
                  cellLoc='center', loc='center',
                  colColours=['#21262d']*6)
table.auto_set_font_size(False)
table.set_fontsize(10)
table.scale(1, 1.6)

# Color the table
tier_for_sub = {s: data[s]['tier'] for s in data}
for key, cell in table.get_celld().items():
    row, col = key
    cell.set_edgecolor('#30363d')
    if row == 0:
        cell.set_facecolor('#21262d')
        cell.set_text_props(color='#58a6ff', fontweight='bold', fontsize=11)
    else:
        sub_name = schedule[row][0]
        if sub_name in tier_for_sub:
            tier = tier_for_sub[sub_name]
            cell.set_facecolor('#0d1117')
            if col == 0:
                cell.set_text_props(color=tier_palette[tier], fontweight='bold')
            else:
                cell.set_text_props(color='#e6edf3')
        else:
            cell.set_facecolor('#0d1117')
            cell.set_text_props(color='#e6edf3')

ax7.set_title('COMPLETE POSTING STRATEGY', fontsize=18, fontweight='bold', pad=20, color='#58a6ff')

plt.savefig('/tmp/jcode_reddit_dashboard.png', dpi=150, bbox_inches='tight',
            facecolor='#0d1117', edgecolor='none')
print("Saved to /tmp/jcode_reddit_dashboard.png")
