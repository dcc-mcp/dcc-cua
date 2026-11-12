// Disposable software lifecycle fixture. It never installs into the system.
using System;
using System.Drawing;
using System.IO;
using System.Windows.Forms;

internal sealed class OwnedWorkflowFixture : ApplicationContext
{
    private readonly string root;
    private readonly string mode;
    private readonly string artifact;
    private Timer progressTimer;
    private int progress;

    private OwnedWorkflowFixture(string fixtureRoot, string fixtureMode)
    {
        root = fixtureRoot;
        mode = fixtureMode;
        artifact = Path.Combine(root, "installed", "fixture.txt");
        Form first = Window("DCC CUA disposable software fixture");
        Label status = Text(first, "Progress: 0%", 24, 24);
        ProgressBar bar = new ProgressBar();
        bar.Location = new Point(24, 130);
        bar.Size = new Size(400, 24);
        bar.AccessibleName = "fixture-progress";
        first.Controls.Add(bar);
        Button action = Button(first, mode == "uninstall" ? "Uninstall" : "Install", 24);
        action.AccessibleName = "fixture-primary";
        Button next = Button(first, "Next", 24);
        next.AccessibleName = "fixture-next";
        next.Visible = false;
        next.Click += delegate
        {
            if (progress != 100) return;
            if (mode == "uninstall")
            {
                if (File.Exists(artifact)) File.Delete(artifact);
                string installed = Path.GetDirectoryName(artifact);
                if (Directory.Exists(installed) && Directory.GetFileSystemEntries(installed).Length == 0)
                    Directory.Delete(installed);
            }
            else
            {
                Directory.CreateDirectory(Path.GetDirectoryName(artifact));
                File.WriteAllText(artifact, "synthetic-installed-v1");
            }
            Record(mode == "uninstall" ? "uninstalled" : "installed");
            ShowCompletion(first);
        };
        action.Click += delegate
        {
            action.Enabled = false;
            Record("primary_clicked");
            if (mode == "hold") return;
            progressTimer = new Timer();
            progressTimer.Interval = 100;
            progressTimer.Tick += delegate
            {
                progress += 25;
                bar.Value = progress;
                status.Text = "Progress: " + progress.ToString() + "%";
                if (progress != 100) return;
                progressTimer.Stop();
                progressTimer.Dispose();
                progressTimer = null;
                if (mode == "fail") { Exit(23); return; }
                Record("progress_complete");
                action.Visible = false;
                next.Visible = true;
            };
            progressTimer.Start();
        };
        Button cancel = Button(first, "Cancel", 240);
        cancel.AccessibleName = "fixture-cancel";
        cancel.Click += delegate { Record("cancel_clicked"); Exit(17); };
        MainForm = first;
        first.Show();
    }

    private Form Window(string title)
    {
        Form form = new Form();
        form.Text = title;
        form.ClientSize = new Size(460, 200);
        form.StartPosition = FormStartPosition.Manual;
        form.Location = new Point(160, 160);
        form.FormBorderStyle = FormBorderStyle.FixedDialog;
        form.MaximizeBox = false;
        form.MinimizeBox = false;
        return form;
    }

    private static Label Text(Form form, string value, int x, int y)
    {
        Label label = new Label();
        label.Text = value;
        label.Location = new Point(x, y);
        label.Size = new Size(400, 30);
        form.Controls.Add(label);
        return label;
    }

    private static Button Button(Form form, string text, int x)
    {
        Button button = new Button();
        button.Text = text;
        button.Location = new Point(x, 70);
        button.Size = new Size(180, 40);
        form.Controls.Add(button);
        return button;
    }

    private void ShowCompletion(Form previous)
    {
        Form next = Window("DCC CUA disposable workflow completed");
        Text(next, "Result verified by fixture; finish to exit", 24, 24);
        Button finish = Button(next, "Finish", 24);
        finish.AccessibleName = "fixture-finish";
        finish.Click += delegate { Record("finish_clicked"); Exit(0); };
        MainForm = next;
        previous.Hide();
        next.Show();
        previous.Close();
        previous.Dispose();
        Record("window_replaced");
    }

    private void Record(string name)
    {
        File.AppendAllText(Path.Combine(root, "ui-events.txt"), name + "\n");
    }

    private void Exit(int code)
    {
        Environment.ExitCode = code;
        if (progressTimer != null) progressTimer.Stop();
        ExitThread();
    }

    [STAThread]
    private static int Main(string[] args)
    {
        if (args.Length != 2 || Array.IndexOf(new string[] { "install", "uninstall", "fail", "hold" }, args[1]) < 0)
            return 64;
        string root = Path.GetFullPath(args[0]);
        string temporary = Path.GetFullPath(Path.GetTempPath()).TrimEnd(Path.DirectorySeparatorChar) + Path.DirectorySeparatorChar;
        if (!(root.StartsWith(temporary, StringComparison.OrdinalIgnoreCase)
              || String.Equals(root.TrimEnd(Path.DirectorySeparatorChar), temporary.TrimEnd(Path.DirectorySeparatorChar), StringComparison.OrdinalIgnoreCase))
            || !Path.GetFileName(root).StartsWith("dcc-cua-owned-workflow-", StringComparison.Ordinal)
            || !Directory.Exists(root)
            || (File.GetAttributes(root) & FileAttributes.ReparsePoint) != 0
            || !File.Exists(Path.Combine(root, ".fixture-root"))
            || File.ReadAllText(Path.Combine(root, ".fixture-root")) != "dcc-cua-owned-workflow-v1")
            return 65;
        string installed = Path.Combine(root, "installed");
        if (Directory.Exists(installed) && (File.GetAttributes(installed) & FileAttributes.ReparsePoint) != 0)
            return 66;
        string artifact = Path.Combine(installed, "fixture.txt");
        if (File.Exists(artifact) && (File.GetAttributes(artifact) & FileAttributes.ReparsePoint) != 0)
            return 66;
        Application.EnableVisualStyles();
        Application.SetCompatibleTextRenderingDefault(false);
        Application.Run(new OwnedWorkflowFixture(root, args[1]));
        return Environment.ExitCode;
    }
}
