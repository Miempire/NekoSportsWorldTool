package org.nekosportsworld.tool;

import android.app.AlertDialog;
import android.app.NativeActivity;
import android.os.Bundle;
import android.text.InputType;
import android.view.View;
import android.view.WindowInsets;
import android.view.WindowManager;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputMethodManager;
import android.widget.EditText;
import android.widget.FrameLayout;

/** Android UI thread owns the real text editor; Rust owns application state. */
public final class MainActivity extends NativeActivity {
    static { System.loadLibrary("nekosportsworldtool"); }
    private AlertDialog editorDialog;
    private AlertDialog deviceInfoDialog;
    private boolean taskActive;
    private boolean resumed;

    private native void nativeSubmitEdit(long id, String value);
    private native void nativeSetInsets(int left, int top, int right, int bottom);
    private native void nativeDeviceInfo(boolean initial, String info, String error);
    private native void nativePickedOsmFile(String path, String error);

    private static final int REQUEST_PICK_OSM = 4101;

    public void requestDeviceInfo(boolean initial) {
        runOnUiThread(() -> {
            android.content.SharedPreferences preferences = getSharedPreferences("device_info", MODE_PRIVATE);
            if (isFinishing() || isDestroyed() || deviceInfoDialog != null) return;
            if (initial && preferences.getBoolean("asked", false)) return;
            String purpose = initial
                ? "允许后会保存到设备身份，用于后续登录和业务请求。"
                : "允许后只填写设备页，点击“保存”后用于后续登录和业务请求。";
            AlertDialog dialog = new AlertDialog.Builder(this)
                .setTitle("读取本机信息")
                .setMessage("是否允许读取本机品牌、型号和 Android 系统版本？\n\n" + purpose
                    + "品牌仅用于设备页展示。"
                    + "\n\n设备 UUID 继续沿用本应用首次生成并保存的值，不会更换。拒绝后仍可手动填写，也可在设备页再次读取。")
                .setPositiveButton("允许读取", (ignored, which) -> {
                    // First-run completion is acknowledged only after Rust saves it.
                    // If the Activity/process exits earlier, the next launch asks again.
                    if (!initial) preferences.edit().putBoolean("asked", true).apply();
                    try {
                        // These fields are read only after the user accepts this dialog.
                        org.json.JSONObject info = new org.json.JSONObject();
                        info.put("manufacturer", android.os.Build.MANUFACTURER);
                        info.put("model", android.os.Build.MODEL);
                        info.put("os_version", android.os.Build.VERSION.RELEASE);
                        nativeDeviceInfo(initial, info.toString(), "");
                    } catch (Exception error) {
                        nativeDeviceInfo(initial, "", "读取本机信息失败，请在设备页重试或手动填写");
                    }
                })
                .setNegativeButton("暂不读取", (ignored, which) -> {
                    preferences.edit().putBoolean("asked", true).apply();
                    nativeDeviceInfo(initial, "", "");
                })
                .create();
            dialog.setOnCancelListener(ignored -> {
                preferences.edit().putBoolean("asked", true).apply();
                nativeDeviceInfo(initial, "", "");
            });
            dialog.setOnDismissListener(ignored -> deviceInfoDialog = null);
            deviceInfoDialog = dialog;
            dialog.show();
        });
    }

    public void completeDeviceInfo() {
        getSharedPreferences("device_info", MODE_PRIVATE).edit().putBoolean("asked", true).apply();
    }

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        // NativeActivity's rendering surface must stay inside system bars.
        View content = findViewById(android.R.id.content);
        content.setOnApplyWindowInsetsListener((view, insets) -> {
            if (android.os.Build.VERSION.SDK_INT >= 30) {
                android.graphics.Insets bars = insets.getInsets(
                    WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
                nativeSetInsets(bars.left, bars.top, bars.right, bars.bottom);
            } else {
                nativeSetInsets(insets.getSystemWindowInsetLeft(), insets.getSystemWindowInsetTop(),
                    insets.getSystemWindowInsetRight(), insets.getSystemWindowInsetBottom());
            }
            return insets;
        });
        content.requestApplyInsets();
        applySystemBarStyle();
    }

    @Override public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (hasFocus) applySystemBarStyle();
    }

    private void applySystemBarStyle() {
        if (android.os.Build.VERSION.SDK_INT >= 30) {
            getWindow().getInsetsController().setSystemBarsAppearance(
                android.view.WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS |
                android.view.WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS,
                android.view.WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS |
                android.view.WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS);
        } else {
            getWindow().getDecorView().setSystemUiVisibility(
                View.SYSTEM_UI_FLAG_LIGHT_STATUS_BAR | View.SYSTEM_UI_FLAG_LIGHT_NAVIGATION_BAR);
        }
    }

    public void openEditor(long id, String initialValue, int kind) {
        runOnUiThread(() -> {
            if (isFinishing() || isDestroyed() || editorDialog != null) return;
            EditText input = new EditText(this);
            input.setId(android.R.id.edit);
            int inputType;
            switch (kind) {
                case 1: inputType = InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD; break;
                case 2: inputType = InputType.TYPE_CLASS_NUMBER | InputType.TYPE_NUMBER_FLAG_SIGNED; break;
                case 3: inputType = InputType.TYPE_CLASS_NUMBER | InputType.TYPE_NUMBER_FLAG_SIGNED | InputType.TYPE_NUMBER_FLAG_DECIMAL; break;
                default: inputType = InputType.TYPE_CLASS_TEXT;
            }
            input.setInputType(inputType);
            input.setSingleLine(true);
            input.setImeOptions(EditorInfo.IME_ACTION_DONE | EditorInfo.IME_FLAG_NO_EXTRACT_UI);
            input.setText(initialValue);
            input.selectAll();
            int margin = Math.round(20 * getResources().getDisplayMetrics().density);
            FrameLayout container = new FrameLayout(this);
            container.setPadding(margin, 0, margin, 0);
            container.addView(input, new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT, FrameLayout.LayoutParams.WRAP_CONTENT));
            AlertDialog dialog = new AlertDialog.Builder(this)
                .setTitle(kind == 1 ? "输入密码" : "编辑内容")
                .setView(container)
                .setPositiveButton("确定", (ignored, which) -> nativeSubmitEdit(id, input.getText().toString()))
                .setNeutralButton("粘贴", null)
                .setNegativeButton("取消", null)
                .create();
            editorDialog = dialog;
            dialog.setOnDismissListener(ignored -> {
                InputMethodManager ime = (InputMethodManager) getSystemService(INPUT_METHOD_SERVICE);
                ime.hideSoftInputFromWindow(input.getWindowToken(), 0);
                editorDialog = null;
            });
            input.setOnEditorActionListener((view, action, event) -> {
                if (action != EditorInfo.IME_ACTION_DONE) return false;
                nativeSubmitEdit(id, input.getText().toString());
                dialog.dismiss();
                return true;
            });
            dialog.setOnShowListener(ignored -> {
                input.requestFocus();
                // 覆盖中性按钮默认的“点击即关闭”，粘贴后对话框保持打开
                dialog.getButton(AlertDialog.BUTTON_NEUTRAL)
                    .setOnClickListener(view -> pasteClipboardInto(input));
                dialog.getWindow().setSoftInputMode(
                    WindowManager.LayoutParams.SOFT_INPUT_STATE_ALWAYS_VISIBLE |
                    WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE);
                input.post(() -> ((InputMethodManager) getSystemService(INPUT_METHOD_SERVICE))
                    .showSoftInput(input, InputMethodManager.SHOW_IMPLICIT));
            });
            dialog.show();
        });
    }

    /** 把剪贴板文本插入输入框光标处（部分输入法长按不出“粘贴”时的兜底）。 */
    private void pasteClipboardInto(EditText input) {
        android.content.ClipboardManager clipboard =
            (android.content.ClipboardManager) getSystemService(CLIPBOARD_SERVICE);
        if (clipboard == null || !clipboard.hasPrimaryClip()) return;
        android.content.ClipData clip = clipboard.getPrimaryClip();
        android.content.ClipData.Item item = clip == null || clip.getItemCount() == 0
            ? null : clip.getItemAt(0);
        CharSequence text = item == null ? null : item.coerceToText(this);
        if (text == null || text.length() == 0) return;
        int start = Math.max(input.getSelectionStart(), 0);
        int end = Math.max(input.getSelectionEnd(), 0);
        input.getText().replace(Math.min(start, end), Math.max(start, end), text.toString());
    }

    /** 系统文件选择器导入 .osm：复制到应用私有目录后把真实路径回传给 Rust。 */
    public void pickOsmFile() {
        runOnUiThread(() -> {
            if (isFinishing() || isDestroyed()) return;
            android.content.Intent intent = new android.content.Intent(android.content.Intent.ACTION_OPEN_DOCUMENT);
            intent.addCategory(android.content.Intent.CATEGORY_OPENABLE);
            intent.setType("*/*");
            intent.putExtra(android.content.Intent.EXTRA_MIME_TYPES, new String[]{
                "text/xml", "application/xml", "text/plain", "application/octet-stream"});
            try {
                startActivityForResult(intent, REQUEST_PICK_OSM);
            } catch (Exception error) {
                nativePickedOsmFile("", "无法打开文件选择器，请手动填写路径");
            }
        });
    }

    @Override protected void onActivityResult(int requestCode, int resultCode, android.content.Intent data) {
        if (requestCode != REQUEST_PICK_OSM) {
            super.onActivityResult(requestCode, resultCode, data);
            return;
        }
        android.net.Uri uri = (resultCode == RESULT_OK && data != null) ? data.getData() : null;
        if (uri == null) {
            nativePickedOsmFile("", "");
            return;
        }
        Thread worker = new Thread(() -> {
            try {
                java.io.File directory = new java.io.File(getFilesDir(), "osm");
                if (!directory.isDirectory() && !directory.mkdirs()) {
                    throw new java.io.IOException("无法创建导入目录");
                }
                java.io.File target = new java.io.File(directory, importedOsmName(uri));
                try (java.io.InputStream input = getContentResolver().openInputStream(uri);
                     java.io.OutputStream output = new java.io.FileOutputStream(target)) {
                    if (input == null) throw new java.io.IOException("无法读取所选文件");
                    byte[] buffer = new byte[64 * 1024];
                    int read;
                    while ((read = input.read(buffer)) > 0) output.write(buffer, 0, read);
                }
                nativePickedOsmFile(target.getAbsolutePath(), "");
            } catch (Exception error) {
                String reason = error.getMessage();
                nativePickedOsmFile("", "导入失败：" + (reason == null ? "未知错误" : reason));
            }
        }, "osm-import");
        worker.start();
    }

    /** 取 SAF 显示名并清洗为安全的 .osm 文件名。 */
    private String importedOsmName(android.net.Uri uri) {
        String name = "";
        try (android.database.Cursor cursor = getContentResolver().query(uri, null, null, null, null)) {
            if (cursor != null && cursor.moveToFirst()) {
                int index = cursor.getColumnIndex(android.provider.OpenableColumns.DISPLAY_NAME);
                if (index >= 0) name = cursor.getString(index);
            }
        } catch (Exception ignored) { }
        if (name == null) name = "";
        name = name.replace('\\', '_').replace('/', '_').replace(':', '_')
            .replace('*', '_').replace('?', '_').replace('"', '_')
            .replace('<', '_').replace('>', '_').replace('|', '_')
            .replace('\n', '_').replace('\r', '_').trim();
        if (name.isEmpty()) name = "import.osm";
        if (!name.toLowerCase(java.util.Locale.ROOT).endsWith(".osm")) name += ".osm";
        return name;
    }

    public void setTaskActive(boolean active) {
        runOnUiThread(() -> {
            taskActive = active;
            updateScreenPolicy();
        });
    }

    public void copyText(String text) {
        runOnUiThread(() -> {
            android.content.ClipboardManager clipboard =
                (android.content.ClipboardManager) getSystemService(CLIPBOARD_SERVICE);
            clipboard.setPrimaryClip(android.content.ClipData.newPlainText("NekoSportsWorldTool", text));
        });
    }

    /** 自更新：把私有目录里已下载的 update.apk 交给系统安装器。 */
    public void installApk(String path) {
        runOnUiThread(() -> {
            try {
                if (path == null || !new java.io.File(path).isFile()) {
                    android.widget.Toast.makeText(this,
                        "安装包不存在，请重新检查更新",
                        android.widget.Toast.LENGTH_LONG).show();
                    return;
                }
                android.content.Intent intent = new android.content.Intent(android.content.Intent.ACTION_VIEW);
                intent.addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK);
                intent.addFlags(android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION);
                intent.setDataAndType(ApkProvider.uriForUpdate(), "application/vnd.android.package-archive");
                startActivity(intent);
            } catch (Exception error) {
                android.widget.Toast.makeText(this,
                    "无法启动安装器，请到系统设置允许本应用安装未知应用后重试",
                    android.widget.Toast.LENGTH_LONG).show();
            }
        });
    }

    /** 当前 APK 的 versionName（与 Release tag 对齐，供更新检查比较）。 */
    public String appVersionName() {
        try {
            return getPackageManager().getPackageInfo(getPackageName(), 0).versionName;
        } catch (Exception error) {
            return "";
        }
    }

    private void updateScreenPolicy() {
        if (taskActive && resumed) getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        else getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
    }

    @Override protected void onResume() {
        super.onResume();
        resumed = true;
        updateScreenPolicy();
    }

    @Override protected void onPause() {
        resumed = false;
        updateScreenPolicy();
        super.onPause();
    }

    @Override protected void onDestroy() {
        if (editorDialog != null) editorDialog.dismiss();
        if (deviceInfoDialog != null) deviceInfoDialog.dismiss();
        taskActive = false;
        updateScreenPolicy();
        super.onDestroy();
    }
}
